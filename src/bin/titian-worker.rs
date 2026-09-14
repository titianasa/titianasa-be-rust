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

use std::sync::Arc;
use std::time::Duration;

use titian_backend_rust::{
    db, observability,
    services::{ai_provider::AIProvider, job_queue},
    Config,
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const SCHEDULE_INTERVAL: Duration = Duration::from_secs(60);
const REAP_INTERVAL: Duration = Duration::from_secs(60);
/// A job locked longer than this without finishing is assumed orphaned
/// (its worker crashed) rather than merely slow. A Pabrik Konten task —
/// generate, QA, up to two repair rounds, apply — takes minutes, not
/// seconds, so this is generous.
const STUCK_AFTER: Duration = Duration::from_secs(30 * 60);
/// Parallel claim loops. Generation spends its time waiting on Vertex,
/// so a few in parallel multiply throughput; more than the Vertex quota
/// allows just trades work for 429 backoff.
const DEFAULT_CONCURRENCY: usize = 4;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    observability::setup();

    let pool = db::connect(&config.database_url).await?;
    let worker_id = format!("{}-{}", hostname(), std::process::id());

    // Same providers the API builds (state.rs). Vertex is required — it is
    // what the generation jobs run on; OpenRouter is optional here.
    let text_ai: Arc<dyn AIProvider> = Arc::new(titian_backend_rust::services::vertex_ai_provider::VertexGeminiProvider::new(config.gcp_project_id.clone(), config.gcp_region.clone()).await?);
    let ai: Arc<dyn AIProvider> = match std::env::var("OPENROUTER_API_KEY") {
        Ok(key) => Arc::new(titian_backend_rust::services::ai_provider::DeepSeekProvider::new(key)),
        Err(_) => Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::failure("OPENROUTER_API_KEY is not set for titian-worker")),
    };
    let concurrency = std::env::var("WORKER_CONCURRENCY").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0).unwrap_or(DEFAULT_CONCURRENCY);
    let ctx = job_queue::JobContext { pool: pool.clone(), config: Arc::new(config), text_ai, ai };
    tracing::info!(worker_id, concurrency, "titian-worker starting");

    let claimers = (0..concurrency).map(|i| tokio::spawn(claim_loop(ctx.clone(), format!("{worker_id}-{i}")))).collect::<Vec<_>>();
    tokio::join!(scheduler_loop(pool.clone()), reaper_loop(pool));
    for claimer in claimers {
        let _ = claimer.await;
    }

    Ok(())
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "worker".to_string())
}

async fn claim_loop(ctx: job_queue::JobContext, worker_id: String) {
    let pool = ctx.pool.clone();
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
            Ok(Some(job)) => run_job(&ctx, job).await,
            Ok(None) => tokio::time::sleep(POLL_INTERVAL).await,
            Err(e) => {
                tracing::error!(error = ?e, "job_queue::claim failed");
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

async fn run_job(ctx: &job_queue::JobContext, job: job_queue::Job) {
    let pool = &ctx.pool;
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
    match (handler.run)(ctx.clone(), job.payload.clone()).await {
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
