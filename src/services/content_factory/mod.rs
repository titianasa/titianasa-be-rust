// Pabrik Konten (ADR-0015) — curriculum content generated from Admin
// Pusat, checked, and applied, instead of from scripts on a developer's
// machine. See migrations/0059 for the data, `runner` for one task's
// life, `admin` for what the dashboard calls.
//
// Kinds today: `bab_plan` (designing the babs of a domain's topics).
// Later kinds (bab content, new topics, tryout sets) plug into the same
// run → task → job → generate/validate/QA/apply shape.

pub mod admin;
pub mod apply;
pub mod benchmark;
pub mod blueprint;
pub mod content;
pub mod content_qa;
pub mod content_runner;
pub mod plan;
pub mod qa;
pub mod runner;
pub mod validate;

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::job_queue::{self, JobContext, JobTypeHandler};

pub const JOB_TYPE: &str = "content_generation";
pub const KINDS: [&str; 2] = ["bab_plan", "bab_content"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RunOptions {
    /// Average judge score (1–5) a topic needs to be applied without review.
    pub qa_threshold: f64,
    /// Repair rounds after the first generation.
    pub max_repairs: i32,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { qa_threshold: 4.2, max_repairs: 2 }
    }
}

fn job_handler(jc: JobContext, payload: serde_json::Value) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
    Box::pin(async move {
        let task_id = payload.get("task_id").and_then(|v| v.as_str()).and_then(|s| s.parse::<Uuid>().ok()).ok_or("payload tanpa task_id")?;
        let kind = sqlx::query_scalar!(r#"select kind from generation_tasks where id = $1"#, task_id).fetch_optional(&jc.pool).await.ok().flatten();
        let result = match kind.as_deref() {
            Some("bab_content") => content_runner::run_bab_content(&jc, task_id).await.map(|_| ()),
            // Default to bab_plan (including an unrecognized/missing
            // kind, which should never happen but must not panic) —
            // it's the only kind that existed before this dispatch did.
            _ => runner::run_bab_plan(&jc, task_id).await.map(|_| ()),
        };
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                // `AppError`'s `Display` is deliberately generic for most
                // variants (e.g. `Internal` always renders as "internal
                // error", by design for API responses) — that's useless
                // on a task the dashboard is supposed to explain a
                // failure from. `{e:?}` goes through anyhow's Debug
                // chain instead, which keeps the real cause (e.g. "agent_qa
                // gagal: HTTP 429 Too Many Requests: ...").
                let message = format!("{e:?}");
                // Recorded on the task so the dashboard shows why; the job
                // queue decides whether to retry.
                let _ = sqlx::query!(r#"update generation_tasks set error = $2, updated_at = now() where id = $1"#, task_id, message).execute(&jc.pool).await;
                let exhausted = sqlx::query_scalar!(r#"select attempt >= max_attempts from jobs where payload->>'task_id' = $1 and status = 'running'"#, task_id.to_string())
                    .fetch_optional(&jc.pool)
                    .await
                    .ok()
                    .flatten()
                    .flatten()
                    .unwrap_or(false);
                if exhausted {
                    let _ = sqlx::query!(r#"update generation_tasks set status = 'failed', updated_at = now() where id = $1"#, task_id).execute(&jc.pool).await;
                    let run_id = sqlx::query_scalar!(r#"select run_id from generation_tasks where id = $1"#, task_id).fetch_optional(&jc.pool).await.ok().flatten();
                    if let Some(run_id) = run_id {
                        let _ = finish_run_if_idle(&jc.pool, run_id).await;
                    }
                }
                Err(message)
            }
        }
    })
}

pub const HANDLER: JobTypeHandler = JobTypeHandler { job_type: JOB_TYPE, run: job_handler };

pub async fn enqueue_task(pool: &PgPool, task_id: Uuid, priority: i32) -> Result<(), AppError> {
    let job_id = job_queue::enqueue(pool, JOB_TYPE, serde_json::json!({"task_id": task_id}), priority, 3).await?;
    sqlx::query!(r#"update generation_tasks set job_id = $2, updated_at = now() where id = $1"#, task_id, job_id).execute(pool).await?;
    Ok(())
}

/// A run is done when none of its tasks can still move on their own.
pub async fn finish_run_if_idle(pool: &PgPool, run_id: Uuid) -> Result<(), AppError> {
    sqlx::query!(
        r#"update generation_runs set status = 'done', finished_at = now(), updated_at = now()
           where id = $1 and status = 'running'
             and not exists (select 1 from generation_tasks where run_id = $1 and status in ('queued', 'generating', 'validating', 'qa'))"#,
        run_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}
