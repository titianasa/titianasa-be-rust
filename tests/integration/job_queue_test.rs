// P40-002 (ADR-0013 §3.1, ADR-0014 §3) — the generic job queue's own
// mechanics: claim (incl. two workers racing for the same job),
// complete, retry-with-backoff-then-fail, reap, and `ensure_scheduled`'s
// idempotency. No HTTP surface exists for this ticket — every test
// calls `job_queue::*` directly against a real (migrated) database.

use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;

use titian_backend_rust::services::job_queue;

async fn status_of(pool: &PgPool, id: uuid::Uuid) -> String {
    sqlx::query_scalar!(r#"select status from jobs where id = $1"#, id).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn enqueue_then_claim_returns_the_same_job_with_attempt_incremented(pool: PgPool) {
    let id = job_queue::enqueue(&pool, "demo_job", json!({"n": 1}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();

    let claimed = job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap().expect("a pending job should be claimable");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.status, "running");
    assert_eq!(claimed.attempt, 1, "claiming spends one attempt");
    assert_eq!(claimed.locked_by.as_deref(), Some("worker-a"));
    assert_eq!(claimed.payload, json!({"n": 1}));
}

#[sqlx::test]
async fn claim_ignores_job_types_the_caller_does_not_handle(pool: PgPool) {
    job_queue::enqueue(&pool, "other_job", json!({}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    let claimed = job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap();
    assert!(claimed.is_none(), "a job of an unrequested type must never be claimed");
}

#[sqlx::test]
async fn lower_priority_number_claims_first(pool: PgPool) {
    let low_urgency = job_queue::enqueue(&pool, "demo_job", json!({"which": "low"}), 200, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    let high_urgency = job_queue::enqueue(&pool, "demo_job", json!({"which": "high"}), 10, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();

    let claimed = job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap().unwrap();
    assert_eq!(claimed.id, high_urgency, "priority 10 must claim before priority 200");
    assert_ne!(claimed.id, low_urgency);
}

#[sqlx::test]
async fn two_workers_racing_never_claim_the_same_job(pool: PgPool) {
    for i in 0..20 {
        job_queue::enqueue(&pool, "demo_job", json!({"i": i}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    }

    // 4 "workers" concurrently draining the same 20 jobs — FOR UPDATE
    // SKIP LOCKED is what this actually proves: without it, two of
    // these tasks could both see the same row as the "next" one before
    // either commits its UPDATE.
    let mut handles = Vec::new();
    for w in 0..4 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let worker_id = format!("worker-{w}");
            let mut claimed_ids = Vec::new();
            loop {
                match job_queue::claim(&pool, &worker_id, &["demo_job"]).await.unwrap() {
                    Some(job) => claimed_ids.push(job.id),
                    None => break,
                }
            }
            claimed_ids
        }));
    }

    let mut all_claimed = Vec::new();
    for h in handles {
        all_claimed.extend(h.await.unwrap());
    }

    assert_eq!(all_claimed.len(), 20, "every job must be claimed exactly once across all workers");
    let unique: std::collections::HashSet<_> = all_claimed.iter().collect();
    assert_eq!(unique.len(), 20, "no job id should appear twice: {all_claimed:?}");
}

#[sqlx::test]
async fn complete_marks_the_job_done_and_releases_the_lock(pool: PgPool) {
    let id = job_queue::enqueue(&pool, "demo_job", json!({}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap();
    job_queue::complete(&pool, id).await.unwrap();

    let row = sqlx::query!(r#"select status, locked_by, finished_at from jobs where id = $1"#, id).fetch_one(&pool).await.unwrap();
    assert_eq!(row.status, "done");
    assert!(row.locked_by.is_none());
    assert!(row.finished_at.is_some());
}

#[sqlx::test]
async fn fail_retries_with_backoff_until_max_attempts_then_marks_failed(pool: PgPool) {
    let id = job_queue::enqueue(&pool, "demo_job", json!({}), job_queue::DEFAULT_PRIORITY, 3).await.unwrap();

    // Attempt 1: claim, fail -> back to pending (retries remain).
    job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap();
    job_queue::fail(&pool, id, "boom 1").await.unwrap();
    let row = sqlx::query!(r#"select status, run_after, last_error, attempt from jobs where id = $1"#, id).fetch_one(&pool).await.unwrap();
    assert_eq!(row.status, "pending");
    assert_eq!(row.attempt, 1);
    assert_eq!(row.last_error.as_deref(), Some("boom 1"));
    assert!(row.run_after > chrono::Utc::now(), "a retried job's run_after must be pushed into the future (backoff)");

    // Force run_after into the past so it's claimable again without
    // waiting out the real backoff in a test.
    sqlx::query!(r#"update jobs set run_after = now() - interval '1 second' where id = $1"#, id).execute(&pool).await.unwrap();

    // Attempt 2: still under max_attempts (3) -> pending again.
    job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap();
    job_queue::fail(&pool, id, "boom 2").await.unwrap();
    assert_eq!(status_of(&pool, id).await, "pending");
    sqlx::query!(r#"update jobs set run_after = now() - interval '1 second' where id = $1"#, id).execute(&pool).await.unwrap();

    // Attempt 3: this WAS the last attempt (max_attempts=3) -> failed permanently.
    job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap();
    job_queue::fail(&pool, id, "boom 3 — out of retries").await.unwrap();
    let row = sqlx::query!(r#"select status, finished_at, last_error, attempt from jobs where id = $1"#, id).fetch_one(&pool).await.unwrap();
    assert_eq!(row.status, "failed");
    assert_eq!(row.attempt, 3);
    assert!(row.finished_at.is_some());
    assert_eq!(row.last_error.as_deref(), Some("boom 3 — out of retries"));

    // A permanently failed job is never claimable again.
    assert!(job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap().is_none());
}

#[sqlx::test]
async fn reap_recovers_a_stuck_job_without_spending_an_attempt(pool: PgPool) {
    let id = job_queue::enqueue(&pool, "demo_job", json!({}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    let claimed = job_queue::claim(&pool, "worker-crashed", &["demo_job"]).await.unwrap().unwrap();
    assert_eq!(claimed.attempt, 1);

    // Simulate the claiming worker having died a while ago — backdate
    // locked_at past the stuck threshold this test uses.
    sqlx::query!(r#"update jobs set locked_at = now() - interval '20 minutes' where id = $1"#, id).execute(&pool).await.unwrap();

    let reaped = job_queue::reap(&pool, Duration::from_secs(15 * 60)).await.unwrap();
    assert_eq!(reaped, 1);

    let row = sqlx::query!(r#"select status, locked_by, locked_at, attempt from jobs where id = $1"#, id).fetch_one(&pool).await.unwrap();
    assert_eq!(row.status, "pending");
    assert!(row.locked_by.is_none());
    assert!(row.locked_at.is_none());
    assert_eq!(row.attempt, 1, "reap must not count as a failed attempt — the job never actually ran to completion or failure");

    // And it's claimable again.
    let reclaimed = job_queue::claim(&pool, "worker-b", &["demo_job"]).await.unwrap().unwrap();
    assert_eq!(reclaimed.id, id);
    assert_eq!(reclaimed.attempt, 2);
}

#[sqlx::test]
async fn reap_leaves_recently_locked_jobs_alone(pool: PgPool) {
    let id = job_queue::enqueue(&pool, "demo_job", json!({}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    job_queue::claim(&pool, "worker-a", &["demo_job"]).await.unwrap();

    let reaped = job_queue::reap(&pool, Duration::from_secs(15 * 60)).await.unwrap();
    assert_eq!(reaped, 0, "a job locked moments ago is presumably still being worked on, not orphaned");
    assert_eq!(status_of(&pool, id).await, "running");
}

#[sqlx::test]
async fn ensure_scheduled_skips_enqueueing_while_one_of_that_type_is_already_pending_or_running(pool: PgPool) {
    let first = job_queue::ensure_scheduled(&pool, "metrics_rollup", json!({}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    assert!(first.is_some(), "nothing pending yet — must enqueue");

    let second = job_queue::ensure_scheduled(&pool, "metrics_rollup", json!({}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    assert!(second.is_none(), "one is already pending — a second tick must not pile up a duplicate");

    let count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from jobs where job_type = 'metrics_rollup'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);

    // Still true while the existing one is running, not just pending.
    job_queue::claim(&pool, "worker-a", &["metrics_rollup"]).await.unwrap();
    let third = job_queue::ensure_scheduled(&pool, "metrics_rollup", json!({}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    assert!(third.is_none(), "a RUNNING job of that type must also block a duplicate enqueue");

    // But once it's done, the next tick is free to schedule again.
    let running_id: uuid::Uuid = sqlx::query_scalar!(r#"select id from jobs where job_type = 'metrics_rollup'"#).fetch_one(&pool).await.unwrap();
    job_queue::complete(&pool, running_id).await.unwrap();
    let fourth = job_queue::ensure_scheduled(&pool, "metrics_rollup", json!({}), job_queue::DEFAULT_PRIORITY, job_queue::DEFAULT_MAX_ATTEMPTS).await.unwrap();
    assert!(fourth.is_some(), "the previous run is done — the next tick should schedule a new one");
}
