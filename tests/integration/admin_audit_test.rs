// P40-001 (ADR-0014 §1) — the /admin skeleton: gate + audit log. DoD:
// platform_admin can open /admin (here: hit its one real endpoint so
// far, GET /admin/audit-log); any other role gets 403; every Manage
// action is recorded.

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
    services::{admin_audit, ai_provider::FakeAIProvider, token},
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
        ai_speaking_room_text_model: "test-model".into(),
        ai_speaking_room_tts_model: "test-tts-model".into(),
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
        ai_writing_evaluation_model: "test-model".into(),
        ai_speaking_evaluation_model: "test-model".into(),
        ai_grammar_evaluation_credit_cost: 1,
        ai_grammar_evaluation_model: "test-model".into(),
        ai_lesson_generation_model: "test-model".into(),
        ai_question_generation_model: "test-model".into(),
        ai_ocr_model: "test-model".into(),
        ai_tts_model: "test-model".into(),
        redis_url: "redis://127.0.0.1:6379".into(),
        collab_checkpoint_interval_seconds: 15,
        ai_live_chat_model: "test-model".into(),
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
async fn platform_admin_can_list_the_audit_log(pool: PgPool) {
    let (admin_uid, admin_token) = insert_user_with_role(&pool, "aud-admin1@example.com", "platform_admin").await;
    admin_audit::record(&pool, Some(admin_uid), "ai_role_settings.updated", "ai_role_settings", None, None, None, Some("test seed")).await.unwrap();

    let app = build_app(pool.clone());
    let (status, body) = get(app, "/admin/audit-log", &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["action"], "ai_role_settings.updated");
    assert_eq!(items[0]["actor_id"], admin_uid.to_string());
    assert_eq!(items[0]["reason"], "test seed");
}

#[sqlx::test]
async fn other_roles_get_403(pool: PgPool) {
    let app = build_app(pool.clone());
    for role in ["student", "teacher", "curriculum_developer", "org_owner", "academic_director"] {
        let (_uid, token) = insert_user_with_role(&pool, &format!("aud-{role}@example.com"), role).await;
        let (status, body) = get(app.clone(), "/admin/audit-log", &token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "role {role} should be forbidden, got {body:?}");
    }
}

#[sqlx::test]
async fn recording_an_action_is_what_list_later_reads_back_including_before_after_snapshots(pool: PgPool) {
    let (admin_uid, admin_token) = insert_user_with_role(&pool, "aud-admin2@example.com", "platform_admin").await;
    let target_id = Uuid::new_v4();
    admin_audit::record(
        &pool,
        Some(admin_uid),
        "ai_model_catalog.toggled",
        "ai_model_catalog",
        Some(target_id),
        Some(serde_json::json!({"enabled": true})),
        Some(serde_json::json!({"enabled": false})),
        Some("model deprecated"),
    )
    .await
    .unwrap();

    let app = build_app(pool.clone());
    let (status, body) = get(app, "/admin/audit-log", &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let row = &body["items"][0];
    assert_eq!(row["target_type"], "ai_model_catalog");
    assert_eq!(row["target_id"], target_id.to_string());
    assert_eq!(row["before"]["enabled"], true);
    assert_eq!(row["after"]["enabled"], false);
    assert_eq!(row["actor_name"], "Test User");
}

#[sqlx::test]
async fn pagination_walks_older_rows_via_next_cursor_without_skipping_or_repeating(pool: PgPool) {
    let (admin_uid, admin_token) = insert_user_with_role(&pool, "aud-admin3@example.com", "platform_admin").await;
    for i in 0..5 {
        admin_audit::record(&pool, Some(admin_uid), &format!("action_{i}"), "thing", None, None, None, None).await.unwrap();
    }

    let app = build_app(pool.clone());
    let (status, page1) = get(app.clone(), "/admin/audit-log?limit=2", &admin_token).await;
    assert_eq!(status, StatusCode::OK);
    let page1_items = page1["items"].as_array().unwrap();
    assert_eq!(page1_items.len(), 2);
    let cursor = page1["next_cursor"].as_str().unwrap();

    let (status, page2) = get(app.clone(), &format!("/admin/audit-log?limit=2&cursor={}", urlencoding_simple(cursor)), &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{page2:?}");
    let page2_items = page2["items"].as_array().unwrap();
    assert_eq!(page2_items.len(), 2);

    let page1_actions: Vec<&str> = page1_items.iter().map(|r| r["action"].as_str().unwrap()).collect();
    let page2_actions: Vec<&str> = page2_items.iter().map(|r| r["action"].as_str().unwrap()).collect();
    assert!(page1_actions.iter().all(|a| !page2_actions.contains(a)), "pages must not overlap: {page1_actions:?} vs {page2_actions:?}");

    // Newest-first: action_4 was recorded last, so it's on page 1.
    assert_eq!(page1_actions[0], "action_4");
}

/// `%3A`/`%2B`-escapes just the handful of characters an rfc3339 cursor
/// (`2026-09-13T00:00:00.123456+00:00_<uuid>`) actually contains that
/// aren't already URL-safe — this is a test helper, not a general
/// encoder.
fn urlencoding_simple(s: &str) -> String {
    s.replace(':', "%3A").replace('+', "%2B")
}
