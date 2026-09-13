// P40-002 (ADR-0013 §3.1, ADR-0014 §3) — the job queue's consumer, a
// separate binary/process from the API server (`cargo run --bin
// titian-worker`), so its deploy/scale/restart is independent of
// request-serving. Connects only to Postgres (`db::connect`), not the
// full `AppState` the API needs (AI providers, R2, Redis, ...) — YAGNI:
// `job_queue::HANDLERS` is empty today, so nothing here needs those
// yet; the moment a real handler (P40-004's `metrics_rollup`, Fase 42's
// content agents) needs one, THAT ticket widens this bootstrap, not
// this one speculatively.
//
// Three independent loops, run concurrently via `tokio::join!`:
//   1. claim loop — the actual work: claim → dispatch to `HANDLERS` →
//      complete/fail. Polls at `POLL_INTERVAL` whenever nothing was
//      claimed (no LISTEN/NOTIFY yet — this queue's volume doesn't
//      need it; revisit if `claim` polling shows up as real load).
//   2. scheduler loop — `job_queue::run_scheduled` on `SCHEDULE_INTERVAL`.
//   3. reaper loop — `job_queue::reap` on `REAP_INTERVAL`, recovering
//      jobs whose worker died mid-run (`STUCK_AFTER`).

use std::time::Duration;

use titian_backend_rust::{db, observability, services::job_queue, Config};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const SCHEDULE_INTERVAL: Duration = Duration::from_secs(60);
const REAP_INTERVAL: Duration = Duration::from_secs(60);
/// A job locked longer than this without finishing is assumed orphaned
/// (its worker crashed) rather than merely slow — generous relative to
/// every handler this queue runs today (daily-aggregate SQL, not
/// long-running AI generation).
const STUCK_AFTER: Duration = Duration::from_secs(15 * 60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    observability::setup();

    let pool = db::connect(&config.database_url).await?;
    let worker_id = format!("{}-{}", hostname(), std::process::id());
    tracing::info!(worker_id, "titian-worker starting");

    tokio::join!(claim_loop(pool.clone(), worker_id), scheduler_loop(pool.clone()), reaper_loop(pool));

    Ok(())
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "worker".to_string())
}

async fn claim_loop(pool: sqlx::PgPool, worker_id: String) {
    let job_types = job_queue::known_job_types();
    loop {
        if job_types.is_empty() {
            // Nothing registered in HANDLERS yet — claiming with an
            // empty type list would just always return None; skip the
            // round-trip and wait for the next tick instead.
            tokio::time::sleep(POLL_INTERVAL).await;
            continue;
        }
        match job_queue::claim(&pool, &worker_id, &job_types).await {
            Ok(Some(job)) => run_job(&pool, job).await,
            Ok(None) => tokio::time::sleep(POLL_INTERVAL).await,
            Err(e) => {
                tracing::error!(error = ?e, "job_queue::claim failed");
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

async fn run_job(pool: &sqlx::PgPool, job: job_queue::Job) {
    let Some(handler) = job_queue::HANDLERS.iter().find(|h| h.job_type == job.job_type) else {
        // Shouldn't happen — claim() only claims types in
        // known_job_types(), which is derived from HANDLERS itself —
        // but a job row inserted with a stale/typo'd job_type before a
        // handler was removed is exactly what this guards against.
        tracing::error!(job_id = %job.id, job_type = %job.job_type, "claimed a job with no registered handler");
        let _ = job_queue::fail(pool, job.id, "no handler registered for this job_type").await;
        return;
    };

    tracing::info!(job_id = %job.id, job_type = %job.job_type, attempt = job.attempt, "running job");
    match (handler.run)(pool.clone(), job.payload.clone()).await {
        Ok(()) => {
            if let Err(e) = job_queue::complete(pool, job.id).await {
                tracing::error!(error = ?e, job_id = %job.id, "job_queue::complete failed");
            }
        }
        Err(msg) => {
            tracing::warn!(job_id = %job.id, job_type = %job.job_type, error = %msg, "job failed");
            if let Err(e) = job_queue::fail(pool, job.id, &msg).await {
                tracing::error!(error = ?e, job_id = %job.id, "job_queue::fail failed");
            }
        }
    }
}

async fn scheduler_loop(pool: sqlx::PgPool) {
    let mut interval = tokio::time::interval(SCHEDULE_INTERVAL);
    loop {
        interval.tick().await;
        if let Err(e) = job_queue::run_scheduled(&pool).await {
            tracing::error!(error = ?e, "job_queue::run_scheduled failed");
        }
    }
}

async fn reaper_loop(pool: sqlx::PgPool) {
    let mut interval = tokio::time::interval(REAP_INTERVAL);
    loop {
        interval.tick().await;
        match job_queue::reap(&pool, STUCK_AFTER).await {
            Ok(0) => {}
            Ok(n) => tracing::warn!(reaped = n, "recovered jobs stuck in running"),
            Err(e) => tracing::error!(error = ?e, "job_queue::reap failed"),
        }
    }
}
