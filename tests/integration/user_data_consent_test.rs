// P39-007 (ADR-0013 "Privasi") — the HTTP surface (`GET`/`POST
// /me/consents`) over `user_data_consent.rs`, which `learning_event_test.rs`
// already covers at the service level.

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
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
    serde_json::from_slice(&bytes).unwrap()
}

async fn send(app: axum::Router, method: Method, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[sqlx::test]
async fn get_consents_defaults_both_kinds_to_not_granted(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "consenthttp1@example.com", "student").await;
    let app = build_app(pool);

    let (status, body) = send(app, Method::GET, "/me/consents", &token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let list = body.as_array().unwrap();
    assert_eq!(list.len(), 2, "both known kinds, even with no row yet");
    assert!(list.iter().all(|c| c["granted"] == json!(false)));
}

#[sqlx::test]
async fn posting_a_consent_grant_is_reflected_on_the_next_get(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "consenthttp2@example.com", "student").await;
    let app = build_app(pool);

    let (status, granted) = send(app.clone(), Method::POST, "/me/consents", &token, json!({"kind": "learning_analytics", "granted": true})).await;
    assert_eq!(status, StatusCode::OK, "{granted:?}");
    assert_eq!(granted["granted"], json!(true));

    let (status, list) = send(app, Method::GET, "/me/consents", &token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let entry = list.as_array().unwrap().iter().find(|c| c["kind"] == json!("learning_analytics")).unwrap();
    assert_eq!(entry["granted"], json!(true));
}

#[sqlx::test]
async fn posting_an_unknown_kind_is_rejected(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "consenthttp3@example.com", "student").await;
    let app = build_app(pool);

    let (status, body) = send(app, Method::POST, "/me/consents", &token, json!({"kind": "not_a_real_kind", "granted": true})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], json!("unknown_consent_kind"));
}
