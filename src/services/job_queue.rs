// P40-002 (ADR-0013 §3.1, ADR-0014 §3) — the generic job queue. Two
// consumers: P40-004's `metrics_rollup` (hourly daily-aggregate
// recompute, registered below) and Fase 42's content agents (not built
// yet). This ticket shipped the queue mechanics themselves —
// `enqueue`/`claim`/`complete`/`fail`/`reap`/`ensure_scheduled` — fully
// generic and proven by tests, plus the worker binary
// (`src/bin/titian-worker.rs`) that runs them. `HANDLERS`/
// `SCHEDULED_JOBS` started empty; P40-004 is the first registrant, one
// line each, no other change to this file's mechanics.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::ai_provider::AIProvider;
use crate::Config;

/// What a job handler gets to work with. The rollup only needs the pool;
/// the Pabrik Konten generator needs Gemini and the role settings too —
/// so the worker builds the same providers the API does, once.
#[derive(Clone)]
pub struct JobContext {
    pub pool: PgPool,
    pub config: Arc<Config>,
    /// Vertex Gemini — text generation and QA.
    pub text_ai: Arc<dyn AIProvider>,
    /// OpenRouter — vision/STT/TTS (unused by current handlers).
    pub ai: Arc<dyn AIProvider>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Job {
    pub id: Uuid,
    pub job_type: String,
    pub payload: serde_json::Value,
    pub priority: i32,
    pub status: String,
    pub attempt: i32,
    pub max_attempts: i32,
    pub run_after: chrono::DateTime<chrono::Utc>,
    pub locked_by: Option<String>,
    pub locked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
    pub cost_tokens: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Lower claims first (0 = most urgent) — matches nice(1), not most
/// scheduler conventions' "higher number wins".
pub const DEFAULT_PRIORITY: i32 = 100;
pub const DEFAULT_MAX_ATTEMPTS: i32 = 5;

pub async fn enqueue(pool: &PgPool, job_type: &str, payload: serde_json::Value, priority: i32, max_attempts: i32) -> Result<Uuid, AppError> {
    let id: Uuid = sqlx::query_scalar!(
        r#"insert into jobs (job_type, payload, priority, max_attempts) values ($1, $2, $3, $4) returning id"#,
        job_type,
        payload,
        priority,
        max_attempts,
    )
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Enqueues a job of `job_type` only if none is currently `pending` or
/// `running` — the idempotency a periodic scheduler needs so a tick
/// firing while the last run is still in flight (or hasn't been
/// claimed yet) doesn't pile up duplicates. Returns `None` when skipped.
/// Deliberately coarse (per job_type, not per "one specific day's
/// rollup") — the one caller this ticket anticipates (P40-004's hourly
/// `metrics_rollup`, which always recomputes "the last 3 days" fresh)
/// has no finer natural key; a caller that needs one can still call
/// `enqueue` directly.
pub async fn ensure_scheduled(pool: &PgPool, job_type: &str, payload: serde_json::Value, priority: i32, max_attempts: i32) -> Result<Option<Uuid>, AppError> {
    let mut tx = pool.begin().await?;
    let existing: Option<Uuid> = sqlx::query_scalar!(r#"select id from jobs where job_type = $1 and status in ('pending', 'running') limit 1"#, job_type)
        .fetch_optional(&mut *tx)
        .await?;
    if existing.is_some() {
        tx.commit().await?;
        return Ok(None);
    }
    let id: Uuid = sqlx::query_scalar!(
        r#"insert into jobs (job_type, payload, priority, max_attempts) values ($1, $2, $3, $4) returning id"#,
        job_type,
        payload,
        priority,
        max_attempts,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// Claims the single most-urgent runnable job whose `job_type` is in
/// `job_types`, or `None` if there isn't one. `FOR UPDATE SKIP LOCKED`
/// inside the CTE is what makes two workers calling this concurrently
/// never claim the same row — the second worker's scan simply skips a
/// row the first has already locked, rather than blocking on it.
/// `attempt` is incremented here, at claim time — claiming a job IS
/// spending one of its attempts, whether it goes on to succeed or fail.
pub async fn claim(pool: &PgPool, worker_id: &str, job_types: &[&str]) -> Result<Option<Job>, AppError> {
    let row = sqlx::query!(
        r#"with next_job as (
             select id from jobs
             where status = 'pending' and run_after <= now() and job_type = any($1::text[])
             order by priority asc, run_after asc
             for update skip locked
             limit 1
           )
           update jobs j
           set status = 'running', locked_by = $2, locked_at = now(), attempt = j.attempt + 1
           from next_job
           where j.id = next_job.id
           returning j.id, j.job_type, j.payload, j.priority, j.status, j.attempt, j.max_attempts,
                     j.run_after, j.locked_by, j.locked_at, j.last_error, j.cost_tokens, j.created_at, j.finished_at"#,
        job_types as &[&str],
        worker_id,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Job {
        id: r.id,
        job_type: r.job_type,
        payload: r.payload,
        priority: r.priority,
        status: r.status,
        attempt: r.attempt,
        max_attempts: r.max_attempts,
        run_after: r.run_after,
        locked_by: r.locked_by,
        locked_at: r.locked_at,
        last_error: r.last_error,
        cost_tokens: r.cost_tokens,
        created_at: r.created_at,
        finished_at: r.finished_at,
    }))
}

pub async fn complete(pool: &PgPool, job_id: Uuid) -> Result<(), AppError> {
    sqlx::query!(r#"update jobs set status = 'done', finished_at = now(), locked_by = null, locked_at = null where id = $1"#, job_id).execute(pool).await?;
    Ok(())
}

/// Staged backoff, capped at 6 doublings (~32 min) so a persistently
/// failing job doesn't wait longer and longer forever: 30s, 60s, 120s,
/// ... capped at 1024s.
fn backoff(attempt: i32) -> Duration {
    let capped_attempt = attempt.clamp(0, 6) as u32;
    Duration::from_secs(30 * 2u64.pow(capped_attempt))
}

/// Records a failed run. If `attempt` (already incremented by `claim`)
/// has reached `max_attempts`, the job is done retrying — `status`
/// becomes `failed` permanently. Otherwise it goes back to `pending`
/// with `run_after` pushed out by `backoff(attempt)`, ready to be
/// claimed again once that passes.
pub async fn fail(pool: &PgPool, job_id: Uuid, error: &str) -> Result<(), AppError> {
    let row = sqlx::query!(r#"select attempt, max_attempts from jobs where id = $1"#, job_id).fetch_one(pool).await?;

    if row.attempt >= row.max_attempts {
        sqlx::query!(
            r#"update jobs set status = 'failed', finished_at = now(), locked_by = null, locked_at = null, last_error = $2 where id = $1"#,
            job_id,
            error,
        )
        .execute(pool)
        .await?;
    } else {
        let run_after = chrono::Utc::now() + chrono::Duration::from_std(backoff(row.attempt)).expect("backoff duration fits in chrono::Duration");
        sqlx::query!(
            r#"update jobs set status = 'pending', run_after = $2, locked_by = null, locked_at = null, last_error = $3 where id = $1"#,
            job_id,
            run_after,
            error,
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Resets jobs stuck in `running` for longer than `stuck_after` back to
/// `pending` — recovery for a worker that crashed (or was killed)
/// mid-job, which otherwise leaves the row locked forever. Does NOT
/// increment `attempt`: the job never actually finished running or
/// failed, it was just orphaned, so this shouldn't spend one of its
/// retries. Returns how many rows were reaped.
pub async fn reap(pool: &PgPool, stuck_after: Duration) -> Result<i64, AppError> {
    let cutoff = chrono::Utc::now() - chrono::Duration::from_std(stuck_after).expect("stuck_after duration fits in chrono::Duration");
    let result = sqlx::query!(
        r#"update jobs set status = 'pending', run_after = now(), locked_by = null, locked_at = null
           where status = 'running' and locked_at < $1"#,
        cutoff,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() as i64)
}

/// One entry per `job_type` `titian-worker` knows how to run. Takes an
/// owned `JobContext` (cheap — pool and providers are `Arc`s) and owned
/// `payload` rather than borrowing, so `run`'s returned future can be
/// `'static` and boxed without lifetime gymnastics at every call site.
pub struct JobTypeHandler {
    pub job_type: &'static str,
    pub run: fn(JobContext, serde_json::Value) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
}

pub const HANDLERS: &[JobTypeHandler] = &[crate::services::metrics_rollup::HANDLER, crate::services::content_factory::HANDLER];

pub fn known_job_types() -> Vec<&'static str> {
    HANDLERS.iter().map(|h| h.job_type).collect()
}

/// One entry per job `titian-worker`'s scheduler keeps alive on every
/// tick of its interval, no more often than `min_interval` apart —
/// P40-004 is the first registrant (`metrics_rollup`, `min_interval`
/// 1 hour, matching that ticket's own "setiap jam").
pub struct ScheduledJob {
    pub job_type: &'static str,
    pub payload: fn() -> serde_json::Value,
    pub priority: i32,
    pub max_attempts: i32,
    pub min_interval: Duration,
}

pub const SCHEDULED_JOBS: &[ScheduledJob] = &[crate::services::metrics_rollup::SCHEDULE];

/// Called on every tick of the worker's scheduler interval. For each
/// `SCHEDULED_JOBS` entry: skip if a job of that type was created
/// within the last `min_interval` (REGARDLESS of its status — even one
/// that already finished still counts, which is what actually enforces
/// the cadence; `ensure_scheduled`'s own pending/running check alone
/// would let a fast-finishing job get re-enqueued on literally the next
/// tick). Otherwise enqueue via `ensure_scheduled` — its concurrent-
/// duplicate guard still applies as a second layer.
pub async fn run_scheduled(pool: &PgPool) -> Result<(), AppError> {
    for job in SCHEDULED_JOBS {
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(job.min_interval).expect("min_interval duration fits in chrono::Duration");
        let recent: Option<Uuid> = sqlx::query_scalar!(r#"select id from jobs where job_type = $1 and created_at > $2 limit 1"#, job.job_type, cutoff).fetch_optional(pool).await?;
        if recent.is_some() {
            continue;
        }
        ensure_scheduled(pool, job.job_type, (job.payload)(), job.priority, job.max_attempts).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::backoff;
    use std::time::Duration;

    #[test]
    fn backoff_doubles_then_caps() {
        assert_eq!(backoff(0), Duration::from_secs(30));
        assert_eq!(backoff(1), Duration::from_secs(60));
        assert_eq!(backoff(2), Duration::from_secs(120));
        assert_eq!(backoff(6), Duration::from_secs(30 * 64));
        // Beyond the cap, stays at the cap rather than overflowing/growing further.
        assert_eq!(backoff(20), backoff(6));
    }
}
