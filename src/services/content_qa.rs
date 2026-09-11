use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::curriculum_constitution::{self, TypedBlock};
use crate::services::module_item::find_content_blocks;
use crate::services::question_schema;

#[derive(Debug, serde::Serialize)]
pub struct QaIssue {
    pub category: String,
    pub message: String,
}

#[derive(Debug, serde::Serialize)]
pub struct QaReport {
    pub passed: bool,
    pub issues: Vec<QaIssue>,
}

fn from_issues(issues: Vec<QaIssue>) -> QaReport {
    let passed = issues.is_empty();
    QaReport { passed, issues }
}

// AppError's UnprocessableEntity detail is already a clear, human
// readable message — QA issues reuse it verbatim instead of re-deriving
// their own wording. Port of content_qa_service.ts's issueMessage.
fn issue_message(err: &AppError) -> String {
    match err {
        AppError::UnprocessableEntity(_, detail) => detail.clone(),
        other => other.to_string(),
    }
}

async fn find_item_has_grammar_concept(pool: &PgPool, item_id: Uuid) -> Result<bool, AppError> {
    let row = sqlx::query_scalar!(
        r#"select c.id from module_item_concepts ic
           inner join concepts c on c.id = ic.concept_id
           where ic.item_id = $1 and c.type = 'grammar'
           limit 1"#,
        item_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

// Port of content_qa_service.ts's runLessonQa, renamed run_item_qa
// (Phase 31). Never blocks the status transition itself ("gerbang
// manusia" principle) — only computes a {passed, issues[]} report.
//
// SCOPE DECISION (Phase 31): the old cefr_mismatch check (embedded
// question's cefr_tag vs. the lesson's containing level's CEFR code) is
// DROPPED here, not silently ported — `levels`/`units` (the CEFR-coded
// tier this check compared against) no longer exist in the
// subject-agnostic module tree, and `questions.cefr_tag` is itself an
// English-specific holdover this migration didn't touch. There is no
// longer a generic "level" concept to compare against. The grammar
// Constitution check below survives unchanged (concepts.type='grammar'
// is still a real, generic concept type).
pub async fn run_item_qa(pool: &PgPool, item_id: Uuid) -> Result<QaReport, AppError> {
    let blocks = find_content_blocks(pool, item_id).await?;
    let mut issues = Vec::new();

    if find_item_has_grammar_concept(pool, item_id).await? {
        let typed: Vec<TypedBlock> = blocks.iter().map(|b| TypedBlock { r#type: b.r#type.clone(), data: b.data.clone() }).collect();
        if let Err(e) = curriculum_constitution::validate_grammar_lesson(&typed) {
            issues.push(QaIssue { category: "grammar_constitution".to_string(), message: issue_message(&e) });
        }
    }

    // Phase 37 — a `quiz` item's question_groups are allowed to be
    // incomplete while drafting (module_item::update_quiz_config only
    // enforces structure, not full field correctness — see
    // quiz_config_schema.rs's header). Full correctness is surfaced
    // HERE instead, same non-blocking "gerbang manusia" contract as
    // every other QA check in this file.
    let quiz_config = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, item_id).fetch_optional(pool).await?.flatten();
    if let Some(quiz_config) = quiz_config {
        for message in crate::services::quiz_config_schema::find_issues(&quiz_config) {
            issues.push(QaIssue { category: "quiz_config".to_string(), message });
        }
    }

    Ok(from_issues(issues))
}

// Port of content_qa_service.ts's runQuestionQa. Same non-persisting
// contract as run_item_qa — question::submit_for_review writes the
// result.
pub async fn run_question_qa(pool: &PgPool, question_id: Uuid) -> Result<QaReport, AppError> {
    let question = sqlx::query!(
        r#"select type, data, correct_answer from questions where id = $1"#,
        question_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("question_not_found"))?;

    let mut issues = Vec::new();
    if let Err(e) = question_schema::validate(&question.r#type, &question.data, &question.correct_answer) {
        issues.push(QaIssue { category: "question_schema".to_string(), message: issue_message(&e) });
    }

    Ok(from_issues(issues))
}
