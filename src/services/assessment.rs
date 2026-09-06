use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::responses::assessment::AssessmentSummary;
use crate::services::permissions::{require_permission, Action, Resource};

pub struct NewAssessment {
    pub r#type: String,
    pub title: String,
    pub config: serde_json::Value,
    pub question_ids: Vec<Uuid>,
}

// POST /assessments — P2-016. No permission row exists for this in
// ADR-0006 (assessment authoring was never designed as its own
// resource) — reuses question_bank:create's tier since bundling
// already-authored questions into an assessment is the same kind of
// authoring work.
pub async fn create_assessment(pool: &PgPool, ctx: &AuthContext, new_assessment: NewAssessment) -> Result<AssessmentSummary, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::Create)?;

    if new_assessment.question_ids.is_empty() {
        return Err(AppError::UnprocessableEntity("invalid_assessment", "question_ids must have at least 1 entry".to_string()));
    }

    let row = sqlx::query!(
        r#"insert into assessments (type, title, config) values ($1, $2, $3) returning id, type, title, config"#,
        new_assessment.r#type,
        new_assessment.title,
        new_assessment.config,
    )
    .fetch_one(pool)
    .await?;

    for (i, question_id) in new_assessment.question_ids.iter().enumerate() {
        sqlx::query!(
            r#"insert into assessment_questions (assessment_id, question_id, order_index) values ($1, $2, $3)"#,
            row.id,
            question_id,
            i as i32,
        )
        .execute(pool)
        .await?;
    }

    Ok(AssessmentSummary {
        id: row.id,
        r#type: row.r#type,
        title: row.title,
        config: row.config,
        question_count: new_assessment.question_ids.len() as i64,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct AttemptQuestion {
    pub id: Uuid,
    pub r#type: String,
    pub data: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
pub struct CreateAttemptResponse {
    pub attempt_id: Uuid,
    pub status: String,
    pub questions: Vec<AttemptQuestion>,
}

// POST /assessments/{id}/attempts
pub async fn create_attempt(pool: &PgPool, ctx: &AuthContext, assessment_id: Uuid) -> Result<CreateAttemptResponse, AppError> {
    require_permission(ctx, Resource::Attempt, Action::Create)?;

    let exists = sqlx::query_scalar!(r#"select id from assessments where id = $1"#, assessment_id).fetch_optional(pool).await?;
    if exists.is_none() {
        return Err(AppError::NotFound("assessment_not_found"));
    }

    let existing = sqlx::query_scalar!(r#"select id from attempts where user_id = $1 and assessment_id = $2 and status = 'in_progress'"#, ctx.user_id, assessment_id)
        .fetch_optional(pool)
        .await?;
    if let Some(existing_id) = existing {
        return Err(AppError::AttemptAlreadyInProgress(existing_id));
    }

    let attempt = sqlx::query!(r#"insert into attempts (user_id, assessment_id) values ($1, $2) returning id, status"#, ctx.user_id, assessment_id).fetch_one(pool).await?;

    let questions = sqlx::query_as!(
        AttemptQuestion,
        r#"select q.id, q.type, q.data from questions q
           inner join assessment_questions aq on aq.question_id = q.id
           where aq.assessment_id = $1
           order by aq.order_index asc"#,
        assessment_id,
    )
    .fetch_all(pool)
    .await?;

    Ok(CreateAttemptResponse { attempt_id: attempt.id, status: attempt.status, questions })
}

// GET /assessments/{id}
pub async fn get_assessment(pool: &PgPool, assessment_id: Uuid) -> Result<AssessmentSummary, AppError> {
    let row = sqlx::query!(r#"select id, type, title, config from assessments where id = $1"#, assessment_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("assessment_not_found"))?;

    let question_count = sqlx::query_scalar!(
        r#"select count(*) as "count!" from assessment_questions where assessment_id = $1"#,
        assessment_id,
    )
    .fetch_one(pool)
    .await?;

    Ok(AssessmentSummary { id: row.id, r#type: row.r#type, title: row.title, config: row.config, question_count })
}

pub struct AttemptRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub assessment_id: Option<Uuid>,
    pub item_id: Option<Uuid>,
    pub status: String,
}

pub async fn find_attempt(pool: &PgPool, id: Uuid) -> Result<Option<AttemptRow>, AppError> {
    let row = sqlx::query_as!(AttemptRow, r#"select id, user_id, assessment_id, item_id, status from attempts where id = $1"#, id).fetch_optional(pool).await?;
    Ok(row)
}

// Small standalone lookup so the submit handler's post-grading XP hook
// doesn't need to re-run get_assessment's full question_count query just
// to learn the assessment's type.
pub async fn find_assessment_type(pool: &PgPool, assessment_id: Uuid) -> Result<Option<String>, AppError> {
    let r#type = sqlx::query_scalar!(r#"select type from assessments where id = $1"#, assessment_id).fetch_optional(pool).await?;
    Ok(r#type)
}

// Shared by submit_attempt, submit_writing_attempt, submit_speaking_attempt.
pub async fn load_submittable_attempt(pool: &PgPool, ctx: &AuthContext, attempt_id: Uuid) -> Result<AttemptRow, AppError> {
    let attempt = find_attempt(pool, attempt_id).await?.ok_or(AppError::NotFound("attempt_not_found"))?;
    if attempt.user_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }
    if attempt.status != "in_progress" {
        return Err(AppError::Conflict("attempt_already_submitted"));
    }
    Ok(attempt)
}

#[derive(Debug, serde::Serialize)]
pub struct CreateLessonAttemptResponse {
    pub attempt_id: Uuid,
    pub status: String,
}

// POST /lessons/{id}/attempts
pub async fn create_lesson_attempt(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<CreateLessonAttemptResponse, AppError> {
    require_permission(ctx, Resource::Attempt, Action::Create)?;

    let item = sqlx::query!(r#"select content_type, status from module_items where id = $1"#, item_id).fetch_optional(pool).await?;
    let Some(item) = item else { return Err(AppError::NotFound("module_item_not_found")) };
    let content_type = item.content_type.as_deref();
    if content_type != Some("writing") && content_type != Some("speaking") {
        return Err(AppError::UnprocessableEntity("invalid_lesson_type", r#"module_item.content_type must be "writing" or "speaking""#.to_string()));
    }
    if item.status != "published" {
        return Err(AppError::ForbiddenWithCode("lesson_not_published"));
    }

    let existing = sqlx::query_scalar!(r#"select id from attempts where user_id = $1 and item_id = $2 and status = 'in_progress'"#, ctx.user_id, item_id).fetch_optional(pool).await?;
    if let Some(existing_id) = existing {
        return Err(AppError::AttemptAlreadyInProgress(existing_id));
    }

    let attempt = sqlx::query!(r#"insert into attempts (user_id, item_id) values ($1, $2) returning id, status"#, ctx.user_id, item_id).fetch_one(pool).await?;
    Ok(CreateLessonAttemptResponse { attempt_id: attempt.id, status: attempt.status })
}

pub async fn submit_lesson_attempt(pool: &PgPool, attempt_id: Uuid, answer_text: &str) -> Result<(), AppError> {
    let answers = serde_json::json!({"text": answer_text});
    sqlx::query!(r#"update attempts set answers = $2, status = 'submitted', submitted_at = now() where id = $1"#, attempt_id, answers).execute(pool).await?;
    Ok(())
}

pub async fn submit_speaking_lesson_attempt(pool: &PgPool, attempt_id: Uuid, audio_asset_id: Uuid) -> Result<(), AppError> {
    let answers = serde_json::json!({"audio_asset_id": audio_asset_id});
    sqlx::query!(r#"update attempts set answers = $2, status = 'submitted', submitted_at = now() where id = $1"#, attempt_id, answers).execute(pool).await?;
    Ok(())
}

pub async fn mark_attempt_evaluated(pool: &PgPool, attempt_id: Uuid, score: f64) -> Result<(), AppError> {
    sqlx::query!(r#"update attempts set status = 'evaluated', score = $2 where id = $1"#, attempt_id, score).execute(pool).await?;
    Ok(())
}

// ASSESSMENT_XP (xp_service.ts) — per-assessment-type XP, distinct from
// xp::skill_xp (per-skill-category). Lives here, not xp.rs, since it's
// only ever consulted from this submit path.
pub fn assessment_xp(assessment_type: &str) -> i32 {
    match assessment_type {
        "mock_exam" => 30,
        "level_assessment" => 30,
        _ => 20, // unit_test and any other/unknown type
    }
}

#[derive(Debug, serde::Serialize)]
pub struct SubmitAttemptResponse {
    pub attempt_id: Uuid,
    pub status: String,
    pub score: f64,
    pub learning_events_created: i64,
}

// POST /attempts/{id}/submit — the plain-assessment grading path
// (unit_test/mock_exam). `assessments.type == 'level_assessment'`
// (ADR-0011 composite Knowledge/Communication scoring) is a genuinely
// separate grading algorithm — deliberately DEFERRED, not
// half-implemented: this function returns a distinct, honest error for
// that type rather than silently mis-grading it with the plain-MCQ
// path.
pub async fn submit_attempt(pool: &PgPool, config: &Config, ctx: &AuthContext, attempt_id: Uuid, answers: &std::collections::HashMap<Uuid, serde_json::Value>) -> Result<SubmitAttemptResponse, AppError> {
    require_permission(ctx, Resource::Attempt, Action::Submit)?;
    let attempt = load_submittable_attempt(pool, ctx, attempt_id).await?;
    let Some(assessment_id) = attempt.assessment_id else { return Err(AppError::Internal(anyhow::anyhow!("attempt has no assessment_id"))) };

    let assessment_type = sqlx::query_scalar!(r#"select type from assessments where id = $1"#, assessment_id).fetch_one(pool).await?;
    if assessment_type == "level_assessment" {
        return Err(AppError::UnprocessableEntity(
            "level_assessment_scoring_not_implemented",
            "Level Assessment composite (Knowledge/Communication) scoring is not yet ported — see phase-30-rust-migration.md".to_string(),
        ));
    }

    struct QuestionRow {
        id: Uuid,
        r#type: String,
        difficulty: f64,
        correct_answer: serde_json::Value,
        data: serde_json::Value,
        explanation: Option<serde_json::Value>,
        version: i32,
    }
    let questions = sqlx::query_as!(
        QuestionRow,
        r#"select q.id, q.type, q.difficulty, q.correct_answer, q.data, q.explanation, q.version
           from questions q inner join assessment_questions aq on aq.question_id = q.id
           where aq.assessment_id = $1 order by aq.order_index asc"#,
        assessment_id,
    )
    .fetch_all(pool)
    .await?;

    let missing: Vec<Uuid> = questions.iter().filter(|q| !answers.contains_key(&q.id)).map(|q| q.id).collect();
    if !missing.is_empty() {
        return Err(AppError::MissingRequiredAnswers(missing));
    }

    let question_ids: Vec<Uuid> = questions.iter().map(|q| q.id).collect();
    let concept_rows = sqlx::query!(r#"select question_id, concept_id from question_concepts where question_id = any($1)"#, &question_ids).fetch_all(pool).await?;
    let mut concept_ids_by_question: std::collections::HashMap<Uuid, Vec<Uuid>> = std::collections::HashMap::new();
    for row in concept_rows {
        concept_ids_by_question.entry(row.question_id).or_default().push(row.concept_id);
    }

    let mut points_possible: f64 = 0.0;
    let mut points_earned: f64 = 0.0;
    let mut touched_concepts: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    let mut concept_correctness: std::collections::HashMap<Uuid, Vec<f64>> = std::collections::HashMap::new();
    let mut events: Vec<(String, serde_json::Value)> = Vec::new();
    let mut question_snapshot = serde_json::Map::new();

    for q in &questions {
        let submitted = answers.get(&q.id).cloned().unwrap_or(serde_json::Value::Null);
        let correct: Option<bool> = if crate::services::grading::is_auto_gradable(&q.r#type) {
            let is_correct_answer = crate::services::grading::is_correct(&q.r#type, &q.correct_answer, &submitted, Some(&q.data));
            points_possible += 1.0;
            if is_correct_answer {
                points_earned += 1.0;
            }
            Some(is_correct_answer)
        } else {
            None
        };

        let concept_ids = concept_ids_by_question.get(&q.id).cloned().unwrap_or_default();
        for concept_id in &concept_ids {
            touched_concepts.insert(*concept_id);
            if let Some(c) = correct {
                concept_correctness.entry(*concept_id).or_default().push(if c { 1.0 } else { 0.0 });
            }
        }

        events.push(("question_answered".to_string(), serde_json::json!({"correct": correct, "difficulty": q.difficulty, "question_id": q.id, "concept_ids": concept_ids})));
        question_snapshot.insert(q.id.to_string(), serde_json::json!({"data": q.data, "correct_answer": q.correct_answer, "explanation": q.explanation, "version": q.version}));
    }

    let score = if points_possible > 0.0 { (points_earned / points_possible * 1000.0).round() / 10.0 } else { 0.0 };
    let answers_json: serde_json::Map<String, serde_json::Value> = answers.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();

    sqlx::query!(
        r#"update attempts set answers = $2, score = $3, status = 'submitted', submitted_at = now(), question_snapshot = $4 where id = $1"#,
        attempt_id,
        serde_json::Value::Object(answers_json),
        score,
        serde_json::Value::Object(question_snapshot),
    )
    .execute(pool)
    .await?;

    let mut learning_events_created = 0i64;
    for (event_type, payload) in &events {
        sqlx::query!(
            r#"insert into learning_events (user_id, event_type, entity_type, entity_id, payload) values ($1, $2, 'question', $3, $4)"#,
            ctx.user_id,
            event_type,
            payload.get("question_id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok()).unwrap(),
            payload,
        )
        .execute(pool)
        .await?;
        learning_events_created += 1;
    }

    for concept_id in &touched_concepts {
        crate::services::mastery::recompute_for_concept(pool, config, ctx.user_id, *concept_id).await?;
    }
    for (concept_id, values) in &concept_correctness {
        let avg = values.iter().sum::<f64>() / values.len() as f64;
        crate::services::frss::record_review(pool, config, ctx.user_id, *concept_id, avg).await?;
    }

    Ok(SubmitAttemptResponse { attempt_id, status: "submitted".to_string(), score, learning_events_created })
}
