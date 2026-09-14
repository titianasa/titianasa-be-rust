// Admin Pusat "Peserta" — DAU/WAU/MAU read the aggregate, "sedang
// online" and the participant detail's IP/device/region read live
// tables (`user_presence`/`user_login_events`), and every detail view
// writes an audit log row (ADR-0014 §5). Numbers were also checked once
// by hand against the real dev database (backfill_metrics + a manual
// SUM/COUNT against learning_events) — these tests are the repeatable
// half of that proof.

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
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
async fn other_roles_get_403_on_every_participants_endpoint(pool: PgPool) {
    let (uid, token) = insert_user_with_role(&pool, "participants-dev@example.com", "curriculum_developer").await;
    let app = build_app(pool.clone());

    for uri in ["/admin/metrics/peserta", "/admin/participants/search?q=a", &format!("/admin/participants/{uid}")] {
        let (status, body) = get(app.clone(), uri, &token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} should be forbidden: {body:?}");
    }
}

#[sqlx::test]
async fn search_finds_by_name_or_email_case_insensitively(pool: PgPool) {
    let (_uid, admin_token) = insert_user_with_role(&pool, "participants-admin1@example.com", "platform_admin").await;
    let (_target_uid, _) = insert_user_with_role(&pool, "budi.santoso@example.com", "student").await;
    sqlx::query!(r#"update users set name = 'Budi Santoso' where email = 'budi.santoso@example.com'"#).execute(&pool).await.unwrap();
    let app = build_app(pool.clone());

    let (status, body) = get(app.clone(), "/admin/participants/search?q=budi", &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let results = body.as_array().unwrap();
    assert!(results.iter().any(|r| r["email"] == "budi.santoso@example.com"), "search by name fragment should find the user: {results:?}");

    let (status, body) = get(app, "/admin/participants/search?q=BUDI.SANTOSO@EXAMPLE.COM", &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let results = body.as_array().unwrap();
    assert!(results.iter().any(|r| r["email"] == "budi.santoso@example.com"), "search by full email, case-insensitive, should also find it: {results:?}");
}

#[sqlx::test]
async fn detail_returns_presence_login_history_and_activity_then_writes_an_audit_log_row(pool: PgPool) {
    let (_uid, admin_token) = insert_user_with_role(&pool, "participants-admin2@example.com", "platform_admin").await;
    let (target_uid, _) = insert_user_with_role(&pool, "participants-target1@example.com", "student").await;

    sqlx::query!(
        r#"insert into user_presence (user_id, last_seen_at, last_ip, last_user_agent, last_device_label) values ($1, now(), '10.0.0.5', 'ua', 'Android · Chrome')"#,
        target_uid,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(r#"insert into user_login_events (user_id, ip, user_agent, device_label) values ($1, '10.0.0.5', 'ua', 'Android · Chrome')"#, target_uid).execute(&pool).await.unwrap();
    sqlx::query!(
        r#"insert into learning_events (user_id, event_type, entity_type, entity_id, payload) values ($1, 'question_answered', 'question', $2, '{"correct": true}')"#,
        target_uid,
        Uuid::new_v4(),
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_app(pool.clone());
    let (status, body) = get(app, &format!("/admin/participants/{target_uid}"), &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["is_online"], true);
    assert_eq!(body["last_ip"], "10.0.0.5");
    assert_eq!(body["last_device_label"], "Android · Chrome");
    assert_eq!(body["login_history"].as_array().unwrap().len(), 1);
    assert_eq!(body["activity"].as_array().unwrap().len(), 1);
    // A private-range IP never triggers the live geolocation lookup at
    // all (see is_private_or_loopback) — keeps this test offline and
    // fast, and proves the skip path itself works.
    assert!(body["region"].is_null());

    let audit_rows = sqlx::query!(r#"select action, target_type, target_id from admin_audit_log where action = 'participant.viewed'"#).fetch_all(&pool).await.unwrap();
    assert_eq!(audit_rows.len(), 1, "every detail view must be audit-logged (ADR-0014 §5)");
    assert_eq!(audit_rows[0].target_type, "user");
    assert_eq!(audit_rows[0].target_id, Some(target_uid));
}

#[sqlx::test]
async fn sedang_online_counts_only_presence_rows_seen_in_the_last_5_minutes(pool: PgPool) {
    let (_uid, admin_token) = insert_user_with_role(&pool, "participants-admin3@example.com", "platform_admin").await;
    let (fresh_uid, _) = insert_user_with_role(&pool, "participants-fresh@example.com", "student").await;
    let (stale_uid, _) = insert_user_with_role(&pool, "participants-stale@example.com", "student").await;

    sqlx::query!(r#"insert into user_presence (user_id, last_seen_at) values ($1, now())"#, fresh_uid).execute(&pool).await.unwrap();
    sqlx::query!(r#"insert into user_presence (user_id, last_seen_at) values ($1, now() - interval '10 minutes')"#, stale_uid).execute(&pool).await.unwrap();

    let app = build_app(pool.clone());
    let (status, body) = get(app, "/admin/metrics/peserta", &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["sedang_online"], 1, "the 10-minute-old row must not count as online: {body:?}");
}

#[sqlx::test]
async fn dau_reads_from_the_aggregate_table_not_a_live_scan(pool: PgPool) {
    let (_uid, admin_token) = insert_user_with_role(&pool, "participants-admin4@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let today = titian_backend_rust::services::metrics_rollup::wib_today();
    sqlx::query!(r#"insert into metrics_daily_participants (day, dau, wau, mau) values ($1, 7, 20, 55)"#, today).execute(&pool).await.unwrap();

    let (status, body) = get(app, &format!("/admin/metrics/peserta?from={today}&to={today}"), &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["dau"]["current"], 7);
    assert_eq!(body["wau"]["current"], 20);
    assert_eq!(body["mau"]["current"], 55);
    assert!(body["dau"]["previous"].is_null(), "no row exists for the previous period — must be \"belum cukup data\": {body:?}");
}
