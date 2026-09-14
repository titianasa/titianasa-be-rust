// P40-004 (ADR-0014 §3) — the daily aggregate rollup. DoD: numbers
// match a direct query against the source tables for sample days;
// triggering the job twice never doubles a number. Verified here with
// controlled seed data (also proven once already against the real dev
// database — every table's totals matched a direct SUM/COUNT exactly,
// and the content snapshot found precisely the one real topic that
// exists in dev today, with a taxonomy breakdown that summed correctly).

use chrono::{NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use titian_backend_rust::services::{job_queue, metrics_rollup};

async fn insert_user(pool: &PgPool, email: &str, created_at: chrono::DateTime<Utc>) -> Uuid {
    sqlx::query_scalar!(
        r#"insert into users (google_id, email, name, created_at) values ($1, $2, 'Test User', $3) returning id"#,
        format!("google-{email}"),
        email,
        created_at,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_order(pool: &PgPool, kind: &str, status: &str, amount_idr: i64, subscription_tier: Option<&str>, user_id: Option<Uuid>, created_at: chrono::DateTime<Utc>) -> Uuid {
    let enrollment_id = if kind == "enrollment" {
        let tutor_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ($1, $2, 'Test Tutor') returning id"#, format!("google-tutor-{}", Uuid::new_v4()), format!("tutor-{}@example.com", Uuid::new_v4()))
            .fetch_one(pool)
            .await
            .unwrap();
        let org_id: Uuid = sqlx::query_scalar!(r#"insert into organizations (name, slug, type) values ('Test Tutor Org', $1, 'tutor_org') returning id"#, format!("tutor-org-{}", Uuid::new_v4())).fetch_one(pool).await.unwrap();
        sqlx::query!(r#"insert into tutor_profiles (user_id, organization_id) values ($1, $2)"#, tutor_id, org_id).execute(pool).await.unwrap();
        let product_id: Uuid = sqlx::query_scalar!(
            r#"insert into learning_products (tutor_id, title, type, price_idr, delivery_mode) values ($1, 'Test Product', 'private', 1, 'self_paced') returning id"#,
            tutor_id,
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let cohort_id: Uuid = sqlx::query_scalar!(r#"insert into cohorts (product_id, name) values ($1, 'Test Cohort') returning id"#, product_id).fetch_one(pool).await.unwrap();
        let id: Uuid = sqlx::query_scalar!(
            r#"insert into enrollments (cohort_id, student_id, status) values ($1, $2, 'active') returning id"#,
            cohort_id,
            user_id.unwrap_or_else(Uuid::new_v4),
        )
        .fetch_one(pool)
        .await
        .unwrap();
        Some(id)
    } else {
        None
    };

    sqlx::query_scalar!(
        r#"insert into orders (kind, status, amount_idr, subscription_tier, user_id, enrollment_id, created_at)
           values ($1, $2, $3, $4, $5, $6, $7) returning id"#,
        kind,
        status,
        amount_idr,
        subscription_tier,
        user_id,
        enrollment_id,
        created_at,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_ai_task(pool: &PgPool, task_type: &str, model: &str, status: &str, tokens_used: Option<i32>, created_at: chrono::DateTime<Utc>) {
    sqlx::query!(
        r#"insert into ai_tasks (task_type, provider, model, prompt_id, status, tokens_used, created_at)
           values ($1, 'vertex', $2, 'test_prompt_v1', $3, $4, $5)"#,
        task_type,
        model,
        status,
        tokens_used,
        created_at,
    )
    .execute(pool)
    .await
    .unwrap();
}

fn wib_midday(day: NaiveDate) -> chrono::DateTime<Utc> {
    // A timestamp safely inside WIB day `day` regardless of the exact
    // hour — noon WIB is 05:00 UTC, nowhere near either UTC day
    // boundary either side of it.
    (day.and_hms_opt(5, 0, 0).unwrap()).and_utc()
}

#[sqlx::test]
async fn sales_rollup_matches_a_direct_query_and_is_idempotent(pool: PgPool) {
    let day = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    let u1 = insert_user(&pool, "sales1@example.com", wib_midday(day)).await;
    let u2 = insert_user(&pool, "sales2@example.com", wib_midday(day)).await;
    insert_order(&pool, "enrollment", "paid", 150_000, None, Some(u1), wib_midday(day)).await;
    insert_order(&pool, "enrollment", "pending", 50_000, None, Some(u2), wib_midday(day)).await;
    insert_order(&pool, "subscription", "paid", 29_000, Some("plus"), Some(u1), wib_midday(day)).await;

    metrics_rollup::rollup_day(&pool, day).await.unwrap();

    let enrollment = sqlx::query!(r#"select orders, paid_orders, revenue_idr from metrics_daily_sales where day = $1 and kind = 'enrollment'"#, day).fetch_one(&pool).await.unwrap();
    assert_eq!(enrollment.orders, 2);
    assert_eq!(enrollment.paid_orders, 1);
    assert_eq!(enrollment.revenue_idr, 150_000);

    let subscription = sqlx::query!(r#"select orders, paid_orders, revenue_idr, subscription_tier from metrics_daily_sales where day = $1 and kind = 'subscription'"#, day).fetch_one(&pool).await.unwrap();
    assert_eq!(subscription.orders, 1);
    assert_eq!(subscription.subscription_tier, "plus");
    assert_eq!(subscription.revenue_idr, 29_000);

    // Trigger it again — DoD: must not double.
    metrics_rollup::rollup_day(&pool, day).await.unwrap();
    metrics_rollup::rollup_day(&pool, day).await.unwrap();
    let enrollment_again = sqlx::query!(r#"select orders, paid_orders, revenue_idr from metrics_daily_sales where day = $1 and kind = 'enrollment'"#, day).fetch_one(&pool).await.unwrap();
    assert_eq!(enrollment_again.orders, 2, "re-running the rollup must not double the count");
    assert_eq!(enrollment_again.revenue_idr, 150_000);

    let row_count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from metrics_daily_sales where day = $1"#, day).fetch_one(&pool).await.unwrap();
    assert_eq!(row_count, 2, "exactly one row per (day, kind, tier) — no duplicate rows from re-running");
}

#[sqlx::test]
async fn ai_rollup_matches_a_direct_query(pool: PgPool) {
    let day = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    insert_ai_task(&pool, "lesson_generation", "gemini-3.8-flash", "done", Some(1000), wib_midday(day)).await;
    insert_ai_task(&pool, "lesson_generation", "gemini-3.8-flash", "done", Some(500), wib_midday(day)).await;
    insert_ai_task(&pool, "lesson_generation", "gemini-3.8-flash", "failed", None, wib_midday(day)).await;

    metrics_rollup::rollup_day(&pool, day).await.unwrap();

    let row = sqlx::query!(r#"select calls, failed, tokens from metrics_daily_ai where day = $1 and task_type = 'lesson_generation'"#, day).fetch_one(&pool).await.unwrap();
    assert_eq!(row.calls, 3);
    assert_eq!(row.failed, 1);
    assert_eq!(row.tokens, 1500);

    let direct: (i64, i64, Option<i64>) = {
        let r = sqlx::query!(r#"select count(*) as "calls!", count(*) filter (where status = 'failed') as "failed!", sum(tokens_used) as tokens from ai_tasks"#).fetch_one(&pool).await.unwrap();
        (r.calls, r.failed, r.tokens.map(|t| t as i64))
    };
    assert_eq!(row.calls as i64, direct.0);
    assert_eq!(row.failed as i64, direct.1);
    assert_eq!(Some(row.tokens), direct.2);
}

#[sqlx::test]
async fn ai_cost_is_estimated_from_the_catalogs_price_once_one_is_set(pool: PgPool) {
    let day = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    insert_ai_task(&pool, "lesson_generation", "gemini-3.8-flash", "done", Some(2_000_000), wib_midday(day)).await;

    // No price configured yet — cost must be honestly 0, not a guess.
    metrics_rollup::rollup_day(&pool, day).await.unwrap();
    let before = sqlx::query!(r#"select cost_idr from metrics_daily_ai where day = $1"#, day).fetch_one(&pool).await.unwrap();
    assert_eq!(before.cost_idr, 0);

    // An admin fills in a price via Pengaturan AI (P40-003) — same
    // catalog row the resolver already reads.
    sqlx::query!(r#"update ai_model_catalog set price_output_per_mtok_idr = 5000 where model_id = 'gemini-3.8-flash'"#).execute(&pool).await.unwrap();

    metrics_rollup::rollup_day(&pool, day).await.unwrap();
    let after = sqlx::query!(r#"select cost_idr from metrics_daily_ai where day = $1"#, day).fetch_one(&pool).await.unwrap();
    // 2,000,000 tokens * Rp5000/M token = Rp10,000 — no code change was
    // needed for this to become accurate once the price existed.
    assert_eq!(after.cost_idr, 10_000);
}

#[sqlx::test]
async fn users_rollup_counts_every_signup_exactly_once_even_with_multiple_orgs(pool: PgPool) {
    let day = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    let solo = insert_user(&pool, "solo@example.com", wib_midday(day)).await;
    let org_member = insert_user(&pool, "orgmember@example.com", wib_midday(day)).await;

    let org_id: Uuid = sqlx::query_scalar!(r#"insert into organizations (name, slug, type) values ('Test Org', 'test-org-metrics', 'school') returning id"#).fetch_one(&pool).await.unwrap();
    sqlx::query!(r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, 'student')"#, org_member, org_id).execute(&pool).await.unwrap();
    sqlx::query!(r#"insert into user_learning_profiles (user_id, jenjang) values ($1, 'SMP-8')"#, org_member).execute(&pool).await.unwrap();
    let _ = solo;

    metrics_rollup::rollup_day(&pool, day).await.unwrap();

    let total: i64 = sqlx::query_scalar!(r#"select coalesce(sum(signups), 0) as "sum!" from metrics_daily_users where day = $1"#, day).fetch_one(&pool).await.unwrap();
    assert_eq!(total, 2, "every signup counted exactly once total, across whatever org/jenjang buckets they fall into");

    let with_org: i32 = sqlx::query_scalar!(r#"select signups from metrics_daily_users where day = $1 and org_id = $2 and jenjang = 'SMP-8'"#, day, org_id).fetch_one(&pool).await.unwrap();
    assert_eq!(with_org, 1);

    let no_org: i32 = sqlx::query_scalar!(
        r#"select signups from metrics_daily_users where day = $1 and org_id = '00000000-0000-0000-0000-000000000000' and jenjang = 'belum_diisi'"#,
        day
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(no_org, 1);
}

#[sqlx::test]
async fn subscriptions_rollup_computes_new_active_and_churned(pool: PgPool) {
    let today = metrics_rollup::wib_today();
    let new_user = insert_user(&pool, "newsub@example.com", Utc::now()).await;
    let churned_user = insert_user(&pool, "churned@example.com", Utc::now()).await;

    // A brand-new paid subscription order today -> counts as "new".
    insert_order(&pool, "subscription", "paid", 29_000, Some("plus"), Some(new_user), Utc::now()).await;
    sqlx::query!(
        r#"insert into subscriptions (user_id, tier, status, current_period_start, current_period_end) values ($1, 'plus', 'active', now(), now() + interval '30 days')"#,
        new_user
    )
    .execute(&pool)
    .await
    .unwrap();

    // A subscription whose period ended TODAY, never renewed -> churned.
    // Anchored one second into today's WIB day rather than "an hour ago":
    // between 00:00 and 01:00 WIB an hour ago is YESTERDAY, and the test
    // failed for that one hour every day.
    sqlx::query!(
        r#"insert into subscriptions (user_id, tier, status, current_period_start, current_period_end)
           values ($1, 'pro', 'active', now() - interval '31 days',
                   ((now() at time zone 'Asia/Jakarta')::date + interval '1 second') at time zone 'Asia/Jakarta')"#,
        churned_user
    )
    .execute(&pool)
    .await
    .unwrap();

    metrics_rollup::rollup_day(&pool, today).await.unwrap();

    let plus = sqlx::query!(r#"select active, new, churned from metrics_daily_subscriptions where day = $1 and tier = 'plus'"#, today).fetch_one(&pool).await.unwrap();
    assert_eq!(plus.new, 1);
    assert_eq!(plus.active, 1);
    assert_eq!(plus.churned, 0);

    let pro = sqlx::query!(r#"select active, new, churned from metrics_daily_subscriptions where day = $1 and tier = 'pro'"#, today).fetch_one(&pool).await.unwrap();
    assert_eq!(pro.new, 0);
    // Day-granularity metric: the period ended only 1 hour ago, still
    // WITHIN today's WIB calendar date — so it counts as active for
    // (part of) today AND churned today. Both true is correct, not a
    // contradiction: the user was active earlier today, then lapsed.
    assert_eq!(pro.active, 1);
    assert_eq!(pro.churned, 1);

    // Every tier always gets a row, even with zero activity.
    let row_count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from metrics_daily_subscriptions where day = $1"#, today).fetch_one(&pool).await.unwrap();
    assert_eq!(row_count, 2);
}

#[sqlx::test]
async fn run_scheduled_does_not_re_enqueue_within_the_min_interval(pool: PgPool) {
    // First tick: nothing exists yet -> enqueues metrics_rollup.
    job_queue::run_scheduled(&pool).await.unwrap();
    let count_after_first: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from jobs where job_type = 'metrics_rollup'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(count_after_first, 1);

    // Claim + complete it immediately, simulating the worker finishing
    // fast — the naive pending/running-only check would let the very
    // next tick enqueue a duplicate.
    let job = job_queue::claim(&pool, "test-worker", &["metrics_rollup"]).await.unwrap().unwrap();
    job_queue::complete(&pool, job.id).await.unwrap();

    // Second tick, moments later: must NOT enqueue again (min_interval is 1 hour).
    job_queue::run_scheduled(&pool).await.unwrap();
    let count_after_second: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from jobs where job_type = 'metrics_rollup'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(count_after_second, 1, "a finished job less than an hour old must still block the next tick from re-enqueueing");
}

#[sqlx::test]
async fn backfill_processes_every_day_from_the_earliest_source_row_and_is_idempotent(pool: PgPool) {
    let far_back = Utc::now() - chrono::Duration::days(5);
    insert_ai_task(&pool, "lesson_generation", "gemini-3.8-flash", "done", Some(100), far_back).await;

    let report = metrics_rollup::backfill(&pool).await.unwrap();
    assert!(report.days_processed >= 5, "must cover at least the 5 days back to the earliest ai_tasks row: {report:?}");

    let total_before: i64 = sqlx::query_scalar!(r#"select coalesce(sum(tokens), 0)::bigint as "sum!" from metrics_daily_ai"#).fetch_one(&pool).await.unwrap();
    assert_eq!(total_before, 100);

    // Running the whole backfill again must not double anything.
    metrics_rollup::backfill(&pool).await.unwrap();
    let total_after: i64 = sqlx::query_scalar!(r#"select coalesce(sum(tokens), 0)::bigint as "sum!" from metrics_daily_ai"#).fetch_one(&pool).await.unwrap();
    assert_eq!(total_after, 100);
}

#[sqlx::test]
async fn content_rollup_finds_real_content_under_a_tahap_folder_and_counts_taxonomy(pool: PgPool) {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('METRICSMTK', 'Metrics Matematika') returning id"#).fetch_one(&pool).await.unwrap();
    let root_id: Uuid = sqlx::query_scalar!(r#"insert into modules (is_folder, title) values (true, 'Semua Mata Pelajaran (Test)') returning id"#).fetch_one(&pool).await.unwrap();
    let subject_folder_id: Uuid = sqlx::query_scalar!(r#"insert into modules (is_folder, parent_id, title) values (true, $1, 'Metrics Matematika') returning id"#, root_id).fetch_one(&pool).await.unwrap();
    let tahap_id: Uuid = sqlx::query_scalar!(r#"insert into modules (is_folder, parent_id, title) values (true, $1, 'Tahap 1 — Dasar') returning id"#, subject_folder_id).fetch_one(&pool).await.unwrap();

    // One topic WITH real content...
    let topic_with_content: Uuid =
        sqlx::query_scalar!(r#"insert into modules (is_folder, parent_id, subject_id, title) values (false, $1, $2, 'Topik Berisi') returning id"#, tahap_id, subject_id).fetch_one(&pool).await.unwrap();
    let lesson_plan = serde_json::json!({"title": "x", "sections": [{"id": "a", "title": "Bab 1", "content": "isi"}]});
    sqlx::query!(r#"insert into module_items (module_id, node_type, title, content_type, lesson_plan) values ($1, 'item', 'Pembahasan', 'article', $2)"#, topic_with_content, lesson_plan)
        .execute(&pool)
        .await
        .unwrap();
    let quiz_config = serde_json::json!({
        "question_groups": [{"questions": [
            {"number": 1, "taxonomy": {"bloom": "c1", "difficulty": "mudah"}},
            {"number": 2, "taxonomy": {"bloom": "c2", "difficulty": "sedang"}},
        ]}]
    });
    sqlx::query!(r#"insert into module_items (module_id, node_type, title, content_type, quiz_config) values ($1, 'item', 'Latihan', 'quiz', $2)"#, topic_with_content, quiz_config)
        .execute(&pool)
        .await
        .unwrap();

    // ...and one topic with NO real content (placeholder only).
    let topic_empty: Uuid =
        sqlx::query_scalar!(r#"insert into modules (is_folder, parent_id, subject_id, title) values (false, $1, $2, 'Topik Kosong') returning id"#, tahap_id, subject_id).fetch_one(&pool).await.unwrap();
    sqlx::query!(r#"insert into module_items (module_id, node_type, title, content_type) values ($1, 'item', 'Artikel 1', 'article')"#, topic_empty).execute(&pool).await.unwrap();

    metrics_rollup::run(&pool).await.unwrap();

    let today = metrics_rollup::wib_today();
    let row = sqlx::query!(
        r#"select topics_total, topics_with_module, topics_with_quiz, questions_total, questions_by_bloom, questions_by_difficulty
           from metrics_daily_content where day = $1 and subject_id = $2 and tahap = 1"#,
        today,
        subject_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.topics_total, 2, "both topics under Tahap 1 counted");
    assert_eq!(row.topics_with_module, 1, "only the topic with a real (non-empty) lesson_plan section");
    assert_eq!(row.topics_with_quiz, 1, "only the topic with taxonomy-tagged questions");
    assert_eq!(row.questions_total, 2);
    assert_eq!(row.questions_by_bloom, serde_json::json!({"c1": 1, "c2": 1}));
    assert_eq!(row.questions_by_difficulty, serde_json::json!({"mudah": 1, "sedang": 1}));

    // Re-running must not double the counts (content is always a
    // full overwrite of today's row, not an accumulation).
    metrics_rollup::run(&pool).await.unwrap();
    let row2 = sqlx::query!(r#"select topics_total, questions_total from metrics_daily_content where day = $1 and subject_id = $2 and tahap = 1"#, today, subject_id).fetch_one(&pool).await.unwrap();
    assert_eq!(row2.topics_total, 2);
    assert_eq!(row2.questions_total, 2);
}
