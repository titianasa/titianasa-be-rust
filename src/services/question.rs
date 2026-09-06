use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::responses::question::{
    ListQuestionsResponse, QuestionBankResponse, QuestionDetailResponse, QuestionStatusResponse, QuestionStemResponse,
    QuestionSummaryResponse,
};
use crate::services::content_qa;
use crate::services::permissions::{is_allowed, require_permission, Action, Resource};
use crate::services::{publish_flow, question_schema};

struct QuestionRow {
    id: Uuid,
    bank_id: Uuid,
    r#type: String,
    difficulty: f64,
    data: serde_json::Value,
    correct_answer: serde_json::Value,
    explanation: Option<serde_json::Value>,
    status: String,
    qa_report: Option<serde_json::Value>,
    cefr_tag: Option<String>,
    skill_category: Option<String>,
    generated_by: String,
}

fn to_detail_response(q: &QuestionRow) -> QuestionDetailResponse {
    QuestionDetailResponse {
        id: q.id,
        bank_id: q.bank_id,
        r#type: q.r#type.clone(),
        difficulty: q.difficulty,
        data: q.data.clone(),
        correct_answer: q.correct_answer.clone(),
        explanation: q.explanation.clone(),
        status: q.status.clone(),
        qa_report: q.qa_report.clone(),
        cefr_tag: q.cefr_tag.clone(),
        skill_category: q.skill_category.clone(),
        generated_by: q.generated_by.clone(),
    }
}

fn to_status_response(q: &QuestionRow) -> QuestionStatusResponse {
    QuestionStatusResponse { id: q.id, status: q.status.clone(), qa_report: q.qa_report.clone() }
}

async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<QuestionRow>, AppError> {
    let row = sqlx::query_as!(
        QuestionRow,
        r#"select id, bank_id, type, difficulty, data, correct_answer, explanation, status, qa_report, cefr_tag, skill_category, generated_by
           from questions where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// Shared by every read path: published questions are an open read,
// non-published ones require view_unpublished — same rule
// content.rs's get_lesson uses for lessons.
async fn find_viewable_question(pool: &PgPool, ctx: &AuthContext, question_id: Uuid) -> Result<QuestionRow, AppError> {
    let question = find_by_id(pool, question_id).await?.ok_or(AppError::NotFound("question_not_found"))?;
    if question.status != "published" && !is_allowed(ctx.role.as_deref(), Resource::QuestionBank, Action::ViewUnpublished) {
        return Err(AppError::ForbiddenWithCode("question_not_published"));
    }
    Ok(question)
}

// GET /question-banks — no permission check, banks have no "View" row
// in the matrix, open read for any authenticated user.
pub async fn list_banks(pool: &PgPool) -> Result<Vec<QuestionBankResponse>, AppError> {
    let rows = sqlx::query_as!(QuestionBankResponse, r#"select id, name from question_banks order by name asc"#)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

// POST /question-banks — same tier as creating a question within a bank.
pub async fn create_bank(pool: &PgPool, ctx: &AuthContext, subject_id: Uuid, name: &str) -> Result<QuestionBankResponse, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::Create)?;
    let row = sqlx::query_as!(
        QuestionBankResponse,
        r#"insert into question_banks (subject_id, name) values ($1, $2) returning id, name"#,
        subject_id,
        name,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /questions/{id} — full detail including correct_answer, the
// authoring/review surface (Studio is never learner-facing).
pub async fn get_question(pool: &PgPool, ctx: &AuthContext, question_id: Uuid) -> Result<QuestionDetailResponse, AppError> {
    let question = find_viewable_question(pool, ctx, question_id).await?;
    Ok(to_detail_response(&question))
}

// GET /questions/{id}/stem — the learner-safe read: just enough to
// *render* a question, deliberately excluding correct_answer/explanation.
pub async fn get_question_stem(pool: &PgPool, ctx: &AuthContext, question_id: Uuid) -> Result<QuestionStemResponse, AppError> {
    let question = find_viewable_question(pool, ctx, question_id).await?;
    Ok(QuestionStemResponse { id: question.id, r#type: question.r#type, data: question.data })
}

pub struct CheckAnswerResult {
    pub correct: bool,
    pub correct_answer: serde_json::Value,
    pub explanation: Option<serde_json::Value>,
    pub skill_category: Option<String>,
    pub rescue_triggered: bool,
    pub rescue_suggested_item_ids: Vec<Uuid>,
}

// POST /questions/{id}/check — 1 question, no attempts row, but NOT
// stateless: writes a learning_event and drives the same mastery/FRSS
// pipeline submit_attempt does, plus Rescue Mode (ADR-0012), which
// submit_attempt does not.
pub async fn check_answer(pool: &PgPool, config: &crate::Config, ctx: &AuthContext, question_id: Uuid, submitted_answer: &serde_json::Value) -> Result<CheckAnswerResult, AppError> {
    let question = find_viewable_question(pool, ctx, question_id).await?;
    // Non-gradable types are hard-`false` here (unlike submit_attempt's
    // `None`/not-graded) — there is no "ungraded" concept for a
    // single-question check.
    let correct = crate::services::grading::is_auto_gradable(&question.r#type) && crate::services::grading::is_correct(&question.r#type, &question.correct_answer, submitted_answer, Some(&question.data));

    let concept_ids: Vec<Uuid> = sqlx::query_scalar!(r#"select concept_id from question_concepts where question_id = $1"#, question_id).fetch_all(pool).await?;

    // Rescue Mode edge-trigger snapshot — BEFORE this answer's event
    // exists, only when wrong.
    let mut was_triggered: std::collections::HashMap<Uuid, bool> = std::collections::HashMap::new();
    if !correct {
        for concept_id in &concept_ids {
            let status = crate::services::rescue::get_rescue_status(pool, config, ctx.user_id, *concept_id).await?;
            was_triggered.insert(*concept_id, status.triggered);
        }
    }

    let payload = serde_json::json!({"correct": correct, "difficulty": question.difficulty, "question_id": question_id, "concept_ids": concept_ids});
    sqlx::query!(r#"insert into learning_events (user_id, event_type, entity_type, entity_id, payload) values ($1, 'question_answered', 'question', $2, $3)"#, ctx.user_id, question_id, payload)
        .execute(pool)
        .await?;

    let mut rescue_triggered = false;
    let mut rescue_suggested_item_ids = Vec::new();
    for concept_id in &concept_ids {
        crate::services::mastery::recompute_for_concept(pool, config, ctx.user_id, *concept_id).await?;
        // Full weight either way, no averaging (only ever 1 question).
        crate::services::frss::record_review(pool, config, ctx.user_id, *concept_id, if correct { 1.0 } else { 0.0 }).await?;
        if !correct && !was_triggered.get(concept_id).copied().unwrap_or(false) {
            let status = crate::services::rescue::get_rescue_status(pool, config, ctx.user_id, *concept_id).await?;
            if status.triggered {
                rescue_triggered = true;
                rescue_suggested_item_ids.extend(status.suggested_item_ids);
            }
        }
    }

    Ok(CheckAnswerResult { correct, correct_answer: question.correct_answer, explanation: question.explanation, skill_category: question.skill_category, rescue_triggered, rescue_suggested_item_ids })
}

const SKILL_CATEGORIES: [&str; 7] = ["vocabulary", "grammar", "reading", "listening", "writing", "speaking", "pronunciation"];

pub struct NewQuestion {
    pub bank_id: Uuid,
    pub r#type: String,
    pub difficulty: f64,
    pub data: serde_json::Value,
    pub correct_answer: serde_json::Value,
    pub explanation: Option<serde_json::Value>,
    pub concept_ids: Vec<Uuid>,
    pub skill_category: Option<String>,
    // P24-001 — 'human' or 'ai' (ai_content.rs's generate_questions).
    pub generated_by: String,
}

// POST /question-banks/{id}/questions — auth: curriculum_developer+.
// Result is always status = draft.
pub async fn create_question(pool: &PgPool, ctx: &AuthContext, new_question: NewQuestion) -> Result<QuestionStatusResponse, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::Create)?;
    question_schema::validate(&new_question.r#type, &new_question.data, &new_question.correct_answer)?;

    if let Some(category) = &new_question.skill_category {
        if !SKILL_CATEGORIES.contains(&category.as_str()) {
            return Err(AppError::UnprocessableEntity(
                "invalid_skill_category",
                format!("skill_category must be one of: {}", SKILL_CATEGORIES.join(", ")),
            ));
        }
    }

    let row = sqlx::query_as!(
        QuestionRow,
        r#"insert into questions (bank_id, type, difficulty, data, correct_answer, explanation, skill_category, generated_by)
           values ($1, $2, $3, $4, $5, $6, $7, $8)
           returning id, bank_id, type, difficulty, data, correct_answer, explanation, status, qa_report, cefr_tag, skill_category, generated_by"#,
        new_question.bank_id,
        new_question.r#type,
        new_question.difficulty,
        new_question.data,
        new_question.correct_answer,
        new_question.explanation,
        new_question.skill_category,
        new_question.generated_by,
    )
    .fetch_one(pool)
    .await?;

    for concept_id in &new_question.concept_ids {
        sqlx::query!(r#"insert into question_concepts (question_id, concept_id) values ($1, $2)"#, row.id, concept_id)
            .execute(pool)
            .await?;
    }

    Ok(to_status_response(&row))
}

pub struct OcrDraftQuestion {
    pub bank_id: Uuid,
    pub r#type: String,
    pub data: serde_json::Value,
    pub correct_answer: serde_json::Value,
    pub explanation: Option<serde_json::Value>,
    pub qa_report: serde_json::Value,
    pub concept_ids: Vec<Uuid>,
}

// Used only by ai_ocr.rs. Bypasses create_question's throw-on-invalid-
// schema gate and skill_category checks entirely — an OCR-extracted
// item with an unrecognized/invalid shape (even type="unknown") is an
// EXPECTED outcome to capture as a flagged draft, not a caller error to
// reject. difficulty is always 0.5 (OCR has no way to estimate it).
pub async fn insert_ocr_draft(pool: &PgPool, draft: OcrDraftQuestion) -> Result<Uuid, AppError> {
    let id = sqlx::query_scalar!(
        r#"insert into questions (bank_id, type, difficulty, data, correct_answer, explanation, generated_by, status, qa_report)
           values ($1, $2, 0.5, $3, $4, $5, 'ai', 'draft', $6) returning id"#,
        draft.bank_id,
        draft.r#type,
        draft.data,
        draft.correct_answer,
        draft.explanation,
        draft.qa_report,
    )
    .fetch_one(pool)
    .await?;

    for concept_id in &draft.concept_ids {
        sqlx::query!(r#"insert into question_concepts (question_id, concept_id) values ($1, $2)"#, id, concept_id).execute(pool).await?;
    }

    Ok(id)
}

// GET /question-banks/{id}/questions?status=&cursor=&limit=
pub async fn list_by_bank(
    pool: &PgPool,
    bank_id: Uuid,
    status: Option<String>,
    cursor: Option<Uuid>,
    limit: Option<i64>,
) -> Result<ListQuestionsResponse, AppError> {
    let limit = limit.unwrap_or(20).clamp(1, 100);
    let rows = sqlx::query_as!(
        QuestionSummaryResponse,
        r#"select id, type, difficulty, status, qa_report, data, skill_category, generated_by from questions
           where bank_id = $1
             and ($2::text is null or status = $2)
             and ($3::uuid is null or id > $3)
           order by id asc
           limit $4"#,
        bank_id,
        status,
        cursor,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;

    let mut rows = rows;
    let next_cursor = if rows.len() > limit as usize { rows.pop().map(|r| r.id) } else { None };

    Ok(ListQuestionsResponse { items: rows, next_cursor })
}

// POST /questions/{id}/submit-review — the QA Agent runs here, a
// finding never blocks the transition.
pub async fn submit_for_review(pool: &PgPool, ctx: &AuthContext, question_id: Uuid) -> Result<QuestionStatusResponse, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::SubmitReview)?;
    let question = find_by_id(pool, question_id).await?.ok_or(AppError::NotFound("question_not_found"))?;
    publish_flow::validate_submit_for_review(&question.status)?;

    let qa_report = content_qa::run_question_qa(pool, question_id).await?;
    let qa_report_json = serde_json::to_value(&qa_report).unwrap();

    let row = sqlx::query_as!(
        QuestionRow,
        r#"update questions set status = 'in_review', qa_report = $2 where id = $1
           returning id, bank_id, type, difficulty, data, correct_answer, explanation, status, qa_report, cefr_tag, skill_category, generated_by"#,
        question_id,
        qa_report_json,
    )
    .fetch_one(pool)
    .await?;

    Ok(to_status_response(&row))
}

async fn update_status(pool: &PgPool, id: Uuid, status: &str) -> Result<QuestionRow, AppError> {
    let row = sqlx::query_as!(
        QuestionRow,
        r#"update questions set status = $2 where id = $1
           returning id, bank_id, type, difficulty, data, correct_answer, explanation, status, qa_report, cefr_tag, skill_category, generated_by"#,
        id,
        status,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// POST /questions/{id}/publish
pub async fn publish(pool: &PgPool, ctx: &AuthContext, question_id: Uuid) -> Result<QuestionStatusResponse, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::Publish)?;
    let question = find_by_id(pool, question_id).await?.ok_or(AppError::NotFound("question_not_found"))?;
    publish_flow::validate_publish(&question.status)?;
    let row = update_status(pool, question_id, "published").await?;
    Ok(to_status_response(&row))
}

// POST /questions/{id}/reject
pub async fn reject(pool: &PgPool, ctx: &AuthContext, question_id: Uuid) -> Result<QuestionStatusResponse, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::Publish)?;
    let question = find_by_id(pool, question_id).await?.ok_or(AppError::NotFound("question_not_found"))?;
    publish_flow::validate_reject(&question.status)?;
    let row = update_status(pool, question_id, "draft").await?;
    Ok(to_status_response(&row))
}

// Port of question_repository.ts's findConceptIdsForQuestions — one
// query for every question_concepts row touching any of question_ids,
// used by the achievement service to resolve concept ids for the
// Improvement check (per-question, since a batch may span concepts).
pub async fn find_concept_ids_for_questions(pool: &PgPool, question_ids: &[Uuid]) -> Result<Vec<Uuid>, AppError> {
    if question_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_scalar!(
        r#"select distinct concept_id from question_concepts where question_id = any($1)"#,
        question_ids,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
