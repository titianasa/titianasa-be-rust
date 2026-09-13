// P40-004 (ADR-0014 §3) — the daily aggregate tables Admin Pusat's
// dashboard (P40-005) reads instead of ever querying a transaction
// table live. Runs as the `metrics_rollup` job (P40-002's queue,
// registered in `job_queue::HANDLERS`/`SCHEDULED_JOBS` at the bottom of
// this file — the only change P40-002 anticipated needing there).
//
// `day` is always a WIB (Asia/Jakarta) calendar date. WIB is a fixed
// UTC+7 offset with no DST, so `Utc::now() + Duration::hours(7)` gives
// the correct wall-clock date in Rust without a timezone-database
// dependency; the SQL side uses `at time zone 'Asia/Jakarta'` for the
// same conversion (Postgres's own zoneinfo, equally correct, and
// self-documenting at the query site).
//
// Every rollup query is `insert ... on conflict (<the table's full
// primary key>) do update` — re-running it for a day that already has
// rows OVERWRITES with the same freshly recomputed truth, never adds a
// duplicate. That is the entire mechanism behind the DoD's "memicu job
// dua kali tidak menggandakan angka".
//
// `metrics_daily_content` is the one exception to "recompute the last 3
// days": curriculum coverage as it looked on a PAST day isn't
// reconstructable — nothing in this schema keeps a change log of the
// module tree — so it is never backfilled per historical day. Every
// run writes/overwrites only TODAY's snapshot; the table becomes a
// running history only from the day this job first ran onward. The
// other four tables (sales/subscriptions/users/ai) have real
// timestamped source data and ARE backfilled (`backfill_metrics`, the
// one-time binary at the bottom of this file).

use std::pin::Pin;
use std::time::Duration;

use chrono::{NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::errors::AppError;

pub fn wib_today() -> NaiveDate {
    (Utc::now() + chrono::Duration::hours(7)).date_naive()
}

async fn rollup_sales(pool: &PgPool, day: NaiveDate) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into metrics_daily_sales (day, kind, subscription_tier, orders, paid_orders, revenue_idr)
           select $1, kind, coalesce(subscription_tier, 'none'),
                  count(*)::int, count(*) filter (where status = 'paid')::int,
                  coalesce(sum(amount_idr) filter (where status = 'paid'), 0)
           from orders
           where (created_at at time zone 'Asia/Jakarta')::date = $1
           group by kind, coalesce(subscription_tier, 'none')
           on conflict (day, kind, subscription_tier) do update set
             orders = excluded.orders, paid_orders = excluded.paid_orders, revenue_idr = excluded.revenue_idr"#,
        day,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// `tier` is a fixed 2-value domain (`subscriptions_tier_check`),
/// enumerated here rather than derived from GROUP BY — same registry
/// idiom as `subscription::tier_price_idr` — so every day gets exactly
/// 2 rows (zero-filled where there's genuinely nothing), never "no row
/// = ambiguous whether that means zero or not-yet-computed".
const SUBSCRIPTION_TIERS: &[&str] = &["plus", "pro"];

async fn rollup_subscriptions(pool: &PgPool, day: NaiveDate) -> Result<(), AppError> {
    for tier in SUBSCRIPTION_TIERS {
        sqlx::query!(
            r#"with first_paid_sub_order as (
                 -- One row per user: their EARLIEST paid subscription
                 -- order ever. Counted as "new" on the day that first
                 -- order landed — subscriptions.current_period_start
                 -- can't be used for this since `activate()` overwrites
                 -- it on every renewal too, not just the first purchase.
                 select distinct on (user_id) user_id,
                        (created_at at time zone 'Asia/Jakarta')::date as d
                 from orders
                 where kind = 'subscription' and status = 'paid' and subscription_tier = $2
                 order by user_id, created_at asc
               ),
               new_count as (
                 select count(*)::int as new from first_paid_sub_order where d = $1
               ),
               -- "Churned on day D" = this user's current subscription
               -- row's period ended on D and, as of NOW, they never
               -- renewed (current_period_end is still that same date,
               -- in the past). subscriptions has no history table —
               -- this is the only honest signal the current schema has
               -- for when a subscription lapsed.
               churn_count as (
                 select count(*)::int as churned from subscriptions
                 where tier = $2
                   and (current_period_end at time zone 'Asia/Jakarta')::date = $1
                   and current_period_end < now()
               ),
               -- "Active on day D" = D fell within this user's CURRENT
               -- known period. Day-granularity, not timestamp: a
               -- subscription that expired an hour into D still counts
               -- as active for D (they WERE, for part of the day) —
               -- the same row can be both active AND churned for the
               -- same D, which is correct, not a contradiction. Past
               -- periods before the latest renewal aren't tracked, so a
               -- user who churned then resubscribed only counts as
               -- active for their most recent period — a known
               -- simplification of the single-row-per-user
               -- `subscriptions` table, not a bug.
               active_count as (
                 select count(*)::int as active from subscriptions
                 where tier = $2 and status = 'active'
                   and (current_period_start at time zone 'Asia/Jakarta')::date <= $1
                   and (current_period_end at time zone 'Asia/Jakarta')::date >= $1
               )
               insert into metrics_daily_subscriptions (day, tier, active, new, churned)
               select $1, $2, active_count.active, new_count.new, churn_count.churned
               from active_count, new_count, churn_count
               on conflict (day, tier) do update set
                 active = excluded.active, new = excluded.new, churned = excluded.churned"#,
            day,
            tier,
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn rollup_users(pool: &PgPool, day: NaiveDate) -> Result<(), AppError> {
    sqlx::query!(
        r#"with signup_orgs as (
             -- DISTINCT ON with organization_id NULLS LAST: a user in
             -- >1 org is counted under one (their lowest org_id),
             -- never double-counted across orgs — signups must sum to
             -- the real headcount. A user in 0 orgs (the common case,
             -- individual self-learner) gets the nil-UUID sentinel.
             select distinct on (u.id) u.id as user_id,
                    (u.created_at at time zone 'Asia/Jakarta')::date as d,
                    coalesce(uor.organization_id, '00000000-0000-0000-0000-000000000000'::uuid) as org_id,
                    coalesce(ulp.jenjang, 'belum_diisi') as jenjang
             from users u
             left join user_organization_roles uor on uor.user_id = u.id
             left join user_learning_profiles ulp on ulp.user_id = u.id
             order by u.id, uor.organization_id nulls last
           )
           insert into metrics_daily_users (day, org_id, jenjang, signups)
           select $1, org_id, jenjang, count(*)::int
           from signup_orgs
           where d = $1
           group by org_id, jenjang
           on conflict (day, org_id, jenjang) do update set signups = excluded.signups"#,
        day,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn rollup_ai(pool: &PgPool, day: NaiveDate) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into metrics_daily_ai (day, task_type, model, calls, failed, tokens, cost_idr)
           select $1, t.task_type, t.model,
                  count(*)::int, count(*) filter (where t.status = 'failed')::int,
                  coalesce(sum(t.tokens_used), 0)::bigint,
                  round(coalesce(sum(t.tokens_used::numeric * coalesce(c.price_output_per_mtok_idr, 0)), 0) / 1000000)::bigint
           from ai_tasks t
           left join ai_model_catalog c on c.model_id = t.model
           where (t.created_at at time zone 'Asia/Jakarta')::date = $1
           group by t.task_type, t.model
           on conflict (day, task_type, model) do update set
             calls = excluded.calls, failed = excluded.failed, tokens = excluded.tokens, cost_idr = excluded.cost_idr"#,
        day,
    )
    .execute(pool)
    .await?;
    Ok(())
}

struct TopicTahap {
    topic_id: uuid::Uuid,
    subject_id: uuid::Uuid,
    tahap: Option<i32>,
}

/// Every topic module (a real, subject-scoped leaf — never a folder)
/// paired with the nearest ancestor folder titled "Tahap N", found by
/// walking UP `parent_id` from each topic. `tahap` is `None` for a
/// topic with no such ancestor (a learning path organized differently,
/// e.g. a reference row under a non-"Tahap"-structured path) — filtered
/// out by the caller, not an error.
async fn topic_tahap_map(pool: &PgPool) -> Result<Vec<TopicTahap>, AppError> {
    let rows = sqlx::query_as!(
        TopicTahap,
        r#"with recursive ancestry as (
             select id as topic_id, id as node_id, parent_id, title, subject_id, 0 as depth
             from modules
             where is_folder = false and subject_id is not null
             union all
             select a.topic_id, m.id, m.parent_id, m.title, a.subject_id, a.depth + 1
             from ancestry a
             join modules m on m.id = a.parent_id
             where a.depth < 10
           )
           select distinct on (topic_id) topic_id as "topic_id!", subject_id as "subject_id!",
                  (regexp_match(title, '^Tahap (\d+)'))[1]::int as tahap
           from ancestry
           where title ~ '^Tahap [0-9]+'
           order by topic_id, depth asc"#
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn rollup_content(pool: &PgPool) -> Result<(), AppError> {
    let day = wib_today();
    let topics = topic_tahap_map(pool).await?;
    let (topic_ids, subject_ids, tahaps): (Vec<_>, Vec<_>, Vec<_>) =
        topics.into_iter().filter_map(|t| t.tahap.map(|tahap| (t.topic_id, t.subject_id, tahap))).fold((vec![], vec![], vec![]), |mut acc, (id, sid, t)| {
            acc.0.push(id);
            acc.1.push(sid);
            acc.2.push(t);
            acc
        });

    if topic_ids.is_empty() {
        return Ok(());
    }

    // Per (subject, tahap): how many topics total, and how many have
    // REAL content (not an empty placeholder shell) of each kind.
    // "Real" article = has at least one section; "real" quiz = has at
    // least one taxonomy-tagged question (jsonb_path_exists with the
    // recursive `$.**` selector, so it doesn't need to know any
    // subtype's exact nesting shape). MUST be `strict $.**`, not lax —
    // found live by a test with a KNOWN expected count: lax mode's `**`
    // visits an array both as the array and as each unwrapped element,
    // silently DOUBLING every match (confirmed directly: lax returned 4
    // taxonomy objects for 2 real questions, strict returned exactly 2).
    // `jsonb_path_exists` itself doesn't care since it's a boolean, but
    // is kept `strict` too for consistency with the counting query below.
    sqlx::query!(
        r#"with topic_map as (
             select * from unnest($2::uuid[], $3::uuid[], $4::int[]) as t(topic_id, subject_id, tahap)
           ),
           topic_content as (
             select tm.subject_id, tm.tahap,
                    bool_or(mi.content_type = 'article' and mi.lesson_plan is not null
                            and jsonb_array_length(coalesce(mi.lesson_plan->'sections', '[]'::jsonb)) > 0) as has_module,
                    bool_or(mi.content_type = 'quiz' and mi.quiz_config is not null
                            and jsonb_path_exists(mi.quiz_config, 'strict $.**.taxonomy')) as has_quiz
             from topic_map tm
             left join module_items mi on mi.module_id = tm.topic_id
             group by tm.topic_id, tm.subject_id, tm.tahap
           )
           insert into metrics_daily_content (day, subject_id, tahap, topics_total, topics_with_module, topics_with_quiz)
           select $1, subject_id, tahap, count(*)::int,
                  count(*) filter (where has_module)::int, count(*) filter (where has_quiz)::int
           from topic_content
           group by subject_id, tahap
           on conflict (day, subject_id, tahap) do update set
             topics_total = excluded.topics_total, topics_with_module = excluded.topics_with_module, topics_with_quiz = excluded.topics_with_quiz"#,
        day,
        &topic_ids as &[uuid::Uuid],
        &subject_ids as &[uuid::Uuid],
        &tahaps as &[i32],
    )
    .execute(pool)
    .await?;

    // Question count + Bloom/difficulty breakdown per (subject, tahap),
    // via the same `strict $.**.taxonomy` extraction (see the doc note
    // above `has_quiz` for why `strict`, not `lax`, is load-bearing here).
    let rows = sqlx::query!(
        r#"with topic_map as (
             select * from unnest($1::uuid[], $2::uuid[], $3::int[]) as t(topic_id, subject_id, tahap)
           )
           select tm.subject_id as "subject_id!", tm.tahap as "tahap!",
                  tax.value->>'bloom' as bloom, tax.value->>'difficulty' as difficulty
           from topic_map tm
           join module_items mi on mi.module_id = tm.topic_id and mi.content_type = 'quiz' and mi.quiz_config is not null
           cross join lateral jsonb_path_query(mi.quiz_config, 'strict $.**.taxonomy') as tax(value)"#,
        &topic_ids as &[uuid::Uuid],
        &subject_ids as &[uuid::Uuid],
        &tahaps as &[i32],
    )
    .fetch_all(pool)
    .await?;

    use std::collections::HashMap;
    let mut by_group: HashMap<(uuid::Uuid, i32), (i64, HashMap<String, i64>, HashMap<String, i64>)> = HashMap::new();
    for row in rows {
        let entry = by_group.entry((row.subject_id, row.tahap)).or_default();
        entry.0 += 1;
        if let Some(b) = row.bloom {
            *entry.1.entry(b).or_insert(0) += 1;
        }
        if let Some(d) = row.difficulty {
            *entry.2.entry(d).or_insert(0) += 1;
        }
    }

    for ((subject_id, tahap), (questions_total, by_bloom, by_difficulty)) in by_group {
        let by_bloom_json = serde_json::to_value(&by_bloom).expect("HashMap<String,i64> serializes");
        let by_difficulty_json = serde_json::to_value(&by_difficulty).expect("HashMap<String,i64> serializes");
        sqlx::query!(
            r#"update metrics_daily_content
               set questions_total = $4, questions_by_bloom = $5, questions_by_difficulty = $6
               where day = $1 and subject_id = $2 and tahap = $3"#,
            day,
            subject_id,
            tahap,
            questions_total as i32,
            by_bloom_json,
            by_difficulty_json,
        )
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Recomputes `day` for every table except `metrics_daily_content`
/// (always today only — see module doc). Public so the backfill binary
/// can call it per historical day without going through the job queue.
pub async fn rollup_day(pool: &PgPool, day: NaiveDate) -> Result<(), AppError> {
    rollup_sales(pool, day).await?;
    rollup_subscriptions(pool, day).await?;
    rollup_users(pool, day).await?;
    rollup_ai(pool, day).await?;
    Ok(())
}

/// The job's actual entry point (registered in `job_queue::HANDLERS`
/// below): the last 3 WIB days for the 4 backfillable tables, plus
/// today's content snapshot.
pub async fn run(pool: &PgPool) -> Result<(), AppError> {
    let today = wib_today();
    for offset in 0..3 {
        rollup_day(pool, today - chrono::Duration::days(offset)).await?;
    }
    rollup_content(pool).await?;
    Ok(())
}

fn job_handler(pool: PgPool, _payload: serde_json::Value) -> Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
    Box::pin(async move { run(&pool).await.map_err(|e| e.to_string()) })
}

pub const JOB_TYPE: &str = "metrics_rollup";

pub const HANDLER: crate::services::job_queue::JobTypeHandler = crate::services::job_queue::JobTypeHandler { job_type: JOB_TYPE, run: job_handler };

pub const SCHEDULE: crate::services::job_queue::ScheduledJob =
    crate::services::job_queue::ScheduledJob { job_type: JOB_TYPE, payload: || serde_json::json!({}), priority: 100, max_attempts: 3, min_interval: Duration::from_secs(3600) };

#[derive(Debug, Serialize)]
pub struct BackfillReport {
    pub earliest_day: Option<NaiveDate>,
    pub days_processed: i64,
}

/// One-time backfill (`src/bin/backfill_metrics.rs`) — every WIB day
/// from the earliest timestamp across the 4 backfillable source tables
/// through today. `metrics_daily_content` is NOT backfilled here (see
/// module doc); its first row is whatever day the recurring job first
/// runs.
pub async fn backfill(pool: &PgPool) -> Result<BackfillReport, AppError> {
    let earliest: Option<chrono::DateTime<Utc>> = sqlx::query_scalar!(
        r#"select least(
             (select min(created_at) from orders),
             (select min(created_at) from users),
             (select min(created_at) from ai_tasks)
           )"#
    )
    .fetch_one(pool)
    .await?;

    let Some(earliest) = earliest else {
        return Ok(BackfillReport { earliest_day: None, days_processed: 0 });
    };

    let earliest_day = (earliest + chrono::Duration::hours(7)).date_naive();
    let today = wib_today();
    let mut day = earliest_day;
    let mut count = 0i64;
    while day <= today {
        rollup_day(pool, day).await?;
        day += chrono::Duration::days(1);
        count += 1;
    }

    Ok(BackfillReport { earliest_day: Some(earliest_day), days_processed: count })
}
