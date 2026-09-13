// P40-005 (ADR-0014 §2) — the 5 dashboard endpoints. DoD: numbers
// match a direct query against the source (also proven once already
// against the real dev database — every figure checked by hand
// matched a manual SUM/COUNT, including the aggregate-vs-live
// distinction for "today"). Permission gate + comparison-to-previous
// logic + the deterministic alert rules are the parts worth a repeatable test.

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use chrono::NaiveDate;
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

use titian_backend_rust::{
    routes,
    services::{ai_provider::FakeAIProvider, token},
    state::AppState,
    Config,
};

const JWT_SECRET: &str = "test-jwt-secret";

fn test_config() -> Config {
    Config {
        bind_addr: "0.0.0.0:0".into(),
        database_url: String::new(),
        jwt_access_secret: JWT_SECRET.into(),
        access_token_ttl_minutes: 15,
        refresh_token_ttl_days: 30,
        frontend_origin: "http://localhost:3001".into(),
        google_client_id: "test-client-id".into(),
        mastery_confidence_threshold: 0.6,
        weakness_score_threshold: 60.0,
        rescue_mode_consecutive_failures: 3,
        review_queue_default_limit: 10,
        review_queue_min_gap_hours: 4,
        attendance_min_duration_ratio: 0.75,
        attendance_late_join_minutes: 10,
        ai_stt_model: "openai/whisper-1".into(),
        ai_tts_default_voice: "af_bella".into(),
        ai_speaking_room_text_model: "gemini-3.8-flash".into(),
        ai_speaking_room_tts_model: "hexgrad/kokoro-82m".into(),
        asset_max_bytes: 25 * 1024 * 1024,
        asset_signed_url_ttl_seconds: 3600,
        asset_presigned_put_ttl_seconds: 900,
        asset_public_signed_url_ttl_seconds: 604_800,
        mastery_lambda: 0.05,
        mastery_n_min: 5.0,
        frss_recalled_threshold: 0.8,
        frss_partial_threshold: 0.4,
        module_completion_min_accuracy: 80.0,
        module_completion_skip_credit_cost: 15,
        ai_writing_evaluation_model: "gemini-3.8-flash".into(),
        ai_speaking_evaluation_model: "gemini-3.8-flash".into(),
        ai_grammar_evaluation_credit_cost: 1,
        ai_grammar_evaluation_model: "gemini-3.8-flash".into(),
        ai_lesson_generation_model: "gemini-3.8-flash".into(),
        ai_question_generation_model: "gemini-3.8-flash".into(),
        ai_ocr_model: "gemini-3.8-flash".into(),
        ai_tts_model: "hexgrad/kokoro-82m".into(),
        redis_url: "redis://127.0.0.1:6379".into(),
        collab_checkpoint_interval_seconds: 15,
        ai_live_chat_model: "gemini-3.8-flash".into(),
        gcp_project_id: "test-gcp-project".into(),
        gcp_region: "us-central1".into(),
        consent_guardian_confirmation_required: false,
    }
}

fn build_app(pool: PgPool) -> axum::Router {
    let state = Arc::new(AppState {
        db: pool,
        redis: redis::Client::open("redis://127.0.0.1:6379").unwrap(),
        config: test_config(),
        google_verifier: titian_backend_rust::services::google_oauth::GoogleTokenVerifier::new(),
        payment_provider: Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        ai_provider: Arc::new(FakeAIProvider::success("{}")),
        text_ai_provider: Arc::new(FakeAIProvider::success("{}")),
        meeting_provider: Arc::new(titian_backend_rust::services::meeting_provider::StubMeetingProvider),
        storage: Arc::new(titian_backend_rust::services::storage::InMemoryStorage::new()),
        canvas_hub: Arc::new(titian_backend_rust::services::canvas_hub::CanvasHub::new()),
        collab_hub: Arc::new(titian_backend_rust::services::collab_hub::CollabHub::new()),
    });
    routes::create_router(state)
}

async fn insert_user_with_role(pool: &PgPool, email: &str, role: &str) -> (Uuid, String) {
    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ($1, $2, $3) returning id"#, format!("google-{email}"), email, "Test User")
        .fetch_one(pool)
        .await
        .unwrap();
    let org_id: Uuid = sqlx::query_scalar!(r#"insert into organizations (name, slug, type) values ('Test Org', $1, 'school') returning id"#, format!("org-{email}"))
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query!(r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, $3)"#, user_id, org_id, role).execute(pool).await.unwrap();
    let token = token::issue_access_token(JWT_SECRET, &user_id.to_string(), email, 15).unwrap();
    (user_id, token)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

async fn get(app: axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().method(Method::GET).uri(uri).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[sqlx::test]
async fn other_roles_get_403_on_every_metrics_endpoint(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-dev@example.com", "curriculum_developer").await;
    let app = build_app(pool.clone());

    for uri in ["/admin/metrics/ringkasan", "/admin/metrics/penjualan", "/admin/metrics/kurikulum", "/admin/metrics/operasional-ai", "/admin/metrics/organisasi"] {
        let (status, body) = get(app.clone(), uri, &token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} should be forbidden: {body:?}");
    }
}

#[sqlx::test]
async fn ringkasan_reads_todays_revenue_live_not_from_the_aggregate(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-admin1@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    // A paid order placed just now — no rollup has run, so it exists
    // ONLY in `orders`, never yet in `metrics_daily_sales`.
    let (buyer_uid, _) = insert_user_with_role(&pool, "metrics-buyer@example.com", "student").await;
    sqlx::query!(r#"insert into orders (kind, status, amount_idr, subscription_tier, user_id) values ('subscription', 'paid', 29000, 'plus', $1)"#, buyer_uid).execute(&pool).await.unwrap();

    let (status, body) = get(app, "/admin/metrics/ringkasan", &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["pendapatan_hari_ini_idr"], 29000, "must be visible immediately, not only after the next hourly rollup");
}

#[sqlx::test]
async fn ringkasan_flags_a_stuck_job_queue(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-admin2@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    sqlx::query!(r#"insert into jobs (job_type, status, run_after) values ('metrics_rollup', 'pending', now() - interval '2 hours')"#).execute(&pool).await.unwrap();

    let (status, body) = get(app, "/admin/metrics/ringkasan", &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let warnings = body["peringatan"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w["kind"] == "job_queue_stuck"), "{warnings:?}");
}

#[sqlx::test]
async fn ringkasan_flags_a_high_ai_failure_rate(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-admin3@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let today = titian_backend_rust::services::metrics_rollup::wib_today();
    // 3 of 10 failed = 30%, above the 10% ADR-0014 §4 threshold.
    sqlx::query!(r#"insert into metrics_daily_ai (day, task_type, model, calls, failed, tokens, cost_idr) values ($1, 'lesson_generation', 'gemini-3.8-flash', 10, 3, 1000, 0)"#, today)
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = get(app, "/admin/metrics/ringkasan", &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let warnings = body["peringatan"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w["kind"] == "ai_failure_rate"), "{warnings:?}");
}

#[sqlx::test]
async fn penjualan_previous_period_is_none_when_no_data_exists_before_it(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-admin4@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let day = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    sqlx::query!(r#"insert into metrics_daily_sales (day, kind, subscription_tier, orders, paid_orders, revenue_idr) values ($1, 'enrollment', 'none', 1, 1, 50000)"#, day).execute(&pool).await.unwrap();

    let (status, body) = get(app, &format!("/admin/metrics/penjualan?from={day}&to={day}"), &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let row = &body["revenue_by_kind"][0];
    assert_eq!(row["revenue"]["current"], 50000);
    assert!(row["revenue"]["previous"].is_null(), "no data exists before the earliest row — must be \"belum cukup data\", not a misleading 0: {row:?}");
}

#[sqlx::test]
async fn penjualan_previous_period_is_a_real_zero_when_data_exists_but_is_quiet(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-admin5@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let quiet_day = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
    let day = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    // Some earlier row exists (so the table's earliest day is before
    // the previous window) but with a DIFFERENT kind, so the specific
    // (day, kind) combo genuinely has zero — not "no data at all".
    sqlx::query!(r#"insert into metrics_daily_sales (day, kind, subscription_tier, orders, paid_orders, revenue_idr) values ($1, 'subscription', 'plus', 1, 1, 29000)"#, quiet_day).execute(&pool).await.unwrap();
    sqlx::query!(r#"insert into metrics_daily_sales (day, kind, subscription_tier, orders, paid_orders, revenue_idr) values ($1, 'enrollment', 'none', 1, 1, 50000)"#, day).execute(&pool).await.unwrap();

    let (status, body) = get(app, &format!("/admin/metrics/penjualan?from={day}&to={day}"), &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let row = &body["revenue_by_kind"][0];
    assert_eq!(row["revenue"]["current"], 50000);
    assert_eq!(row["revenue"]["previous"], 0, "the table has history before this window, so a quiet previous period is a real, comparable zero");
}

#[sqlx::test]
async fn penjualan_mrr_is_active_subscriptions_times_tier_price(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-admin6@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let today = titian_backend_rust::services::metrics_rollup::wib_today();
    sqlx::query!(r#"insert into metrics_daily_subscriptions (day, tier, active, new, churned) values ($1, 'plus', 3, 0, 0)"#, today).execute(&pool).await.unwrap();
    sqlx::query!(r#"insert into metrics_daily_subscriptions (day, tier, active, new, churned) values ($1, 'pro', 2, 0, 0)"#, today).execute(&pool).await.unwrap();

    let (status, body) = get(app, "/admin/metrics/penjualan", &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    // 3 * Rp29.000 (plus) + 2 * Rp79.000 (pro) = 87.000 + 158.000 = 245.000
    assert_eq!(body["mrr_idr"], 245_000);
}

#[sqlx::test]
async fn kurikulum_compares_actual_taxonomy_against_the_calibrated_target(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-admin7@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('METRICSSUBJ', 'Uji Kurikulum') returning id"#).fetch_one(&pool).await.unwrap();
    let today = titian_backend_rust::services::metrics_rollup::wib_today();
    sqlx::query!(
        r#"insert into metrics_daily_content (day, subject_id, tahap, topics_total, topics_with_module, topics_with_quiz, questions_total, questions_by_bloom, questions_by_difficulty)
           values ($1, $2, 1, 10, 5, 5, 20, '{"c1": 20}'::jsonb, '{"mudah": 20}'::jsonb)"#,
        today,
        subject_id,
    )
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = get(app, "/admin/metrics/kurikulum", &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let subject = body["subjects"].as_array().unwrap().iter().find(|s| s["subject_id"] == subject_id.to_string()).expect("subject present");
    let tahap1 = &subject["tahaps"][0];
    assert_eq!(tahap1["questions_by_bloom"], serde_json::json!({"c1": 20}));
    // Tahap 1 = SD -> target bloom weights [20,35,30,15,0,0] over 20 questions.
    assert_eq!(tahap1["questions_by_bloom_target"]["c1"], 4);
    assert_eq!(tahap1["questions_by_bloom_target"]["c2"], 7);
}

#[sqlx::test]
async fn operasional_ai_reports_queue_health(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "metrics-admin8@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    sqlx::query!(r#"insert into jobs (job_type, status) values ('metrics_rollup', 'pending')"#).execute(&pool).await.unwrap();
    sqlx::query!(r#"insert into jobs (job_type, status) values ('metrics_rollup', 'running')"#).execute(&pool).await.unwrap();
    sqlx::query!(r#"insert into jobs (job_type, status, finished_at) values ('metrics_rollup', 'failed', now())"#).execute(&pool).await.unwrap();

    let (status, body) = get(app, "/admin/metrics/operasional-ai", &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["queue_health"]["pending"], 1);
    assert_eq!(body["queue_health"]["running"], 1);
    assert_eq!(body["queue_health"]["failed_last_24h"], 1);
}

#[sqlx::test]
async fn organisasi_counts_members_and_excludes_the_platform_pseudo_org(pool: PgPool) {
    let (_admin_uid, token) = insert_user_with_role(&pool, "metrics-admin9@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let real_org_id: Uuid = sqlx::query_scalar!(r#"insert into organizations (name, slug, type) values ('Sekolah Uji', 'sekolah-uji-metrics', 'school') returning id"#).fetch_one(&pool).await.unwrap();
    let (student_uid, _) = insert_user_with_role(&pool, "metrics-student@example.com", "student").await;
    sqlx::query!(r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, 'student')"#, student_uid, real_org_id).execute(&pool).await.unwrap();
    sqlx::query!(r#"insert into organizations (name, slug, type) values ('Platform', 'platform-pseudo-org', 'platform')"#).execute(&pool).await.unwrap();

    let (status, body) = get(app, "/admin/metrics/organisasi", &token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let orgs = body["organizations"].as_array().unwrap();
    assert!(!orgs.iter().any(|o| o["type"] == "platform"), "the platform pseudo-org must never appear as a customer organization: {orgs:?}");
    let real = orgs.iter().find(|o| o["id"] == real_org_id.to_string()).expect("real org present");
    assert_eq!(real["member_count"], 1);
}
