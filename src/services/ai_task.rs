use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

// Port of ai_task_repository.ts. Recorded `provider` is always the
// literal "deepseek" across every call site (a routing-table label, not
// derived from the model string). Only ever written once, already
// carrying its final status ("done"/"failed") — never updated in place.
// `insert_failed` never sets tokens_used (stays NULL): a failed
// validation never charges credit, and if the provider call itself
// errored there may be no usable token count either.

pub async fn insert_done(pool: &PgPool, id: Uuid, user_id: Uuid, task_type: &str, provider: &str, model: &str, prompt_id: &str, tokens_used: Option<i32>) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into ai_tasks (id, user_id, task_type, provider, model, prompt_id, tokens_used, status) values ($1, $2, $3, $4, $5, $6, $7, 'done')"#,
        id,
        user_id,
        task_type,
        provider,
        model,
        prompt_id,
        tokens_used,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_failed(pool: &PgPool, id: Uuid, user_id: Uuid, task_type: &str, provider: &str, model: &str, prompt_id: &str) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into ai_tasks (id, user_id, task_type, provider, model, prompt_id, status) values ($1, $2, $3, $4, $5, $6, 'failed')"#,
        id,
        user_id,
        task_type,
        provider,
        model,
        prompt_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}
