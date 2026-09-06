use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

// Port of rubric_repository.ts + evaluation_repository.ts (the parts
// writing/speaking evaluation need — no separate rubric/evaluation
// handler exists, these are internal plumbing only).

pub struct Rubric {
    pub id: Uuid,
}

// Idempotent: INSERT ... ON CONFLICT (id) DO NOTHING, falling back to a
// SELECT if the insert was skipped (a 2nd caller racing to ensure the
// same fixed-id rubric row).
pub async fn ensure_rubric(pool: &PgPool, id: Uuid, name: &str, criteria: serde_json::Value) -> Result<Rubric, AppError> {
    let inserted = sqlx::query_scalar!(r#"insert into rubrics (id, name, criteria) values ($1, $2, $3) on conflict (id) do nothing returning id"#, id, name, criteria)
        .fetch_optional(pool)
        .await?;
    if let Some(id) = inserted {
        return Ok(Rubric { id });
    }
    let id = sqlx::query_scalar!(r#"select id from rubrics where id = $1"#, id)
        .fetch_one(pool)
        .await?;
    Ok(Rubric { id })
}

pub struct InsertedEvaluation {
    pub id: Uuid,
}

pub async fn insert_evaluation(pool: &PgPool, attempt_id: Uuid, evaluator_type: &str, rubric_id: Uuid, scores: serde_json::Value, evidence: serde_json::Value, question_id: Option<Uuid>) -> Result<InsertedEvaluation, AppError> {
    let id = sqlx::query_scalar!(
        r#"insert into evaluations (attempt_id, evaluator_type, rubric_id, scores, evidence, question_id) values ($1, $2, $3, $4, $5, $6) returning id"#,
        attempt_id,
        evaluator_type,
        rubric_id,
        scores,
        evidence,
        question_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(InsertedEvaluation { id })
}

pub struct FeedbackItemInput {
    pub content: String,
    pub position: Option<serde_json::Value>,
}

pub async fn insert_many_feedback(pool: &PgPool, evaluation_id: Uuid, items: &[FeedbackItemInput]) -> Result<(), AppError> {
    for item in items {
        sqlx::query!(
            r#"insert into feedback (evaluation_id, type, content, position) values ($1, 'annotation', $2, $3)"#,
            evaluation_id,
            item.content,
            item.position,
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}
