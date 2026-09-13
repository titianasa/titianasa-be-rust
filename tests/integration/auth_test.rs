use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

use titian_backend_rust::{routes, services::google_oauth::GoogleTokenVerifier, state::AppState, Config};

const TEST_KID: &str = "test-key-1";
const TEST_CLIENT_ID: &str = "test-client-id";

fn test_config() -> Config {
    Config {
        bind_addr: "0.0.0.0:0".into(),
        database_url: String::new(), // unused — pool is injected directly by #[sqlx::test]
        jwt_access_secret: "test-jwt-secret".into(),
        access_token_ttl_minutes: 15,
        refresh_token_ttl_days: 30,
        frontend_origin: "http://localhost:3001".into(),
        google_client_id: TEST_CLIENT_ID.into(),
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
        ai_live_chat_model: "~deepseek/deepseek-v4-flash-latest".into(),
        gcp_project_id: "test-gcp-project".into(),
        gcp_region: "us-central1".into(),
    }
}

fn build_app(pool: PgPool) -> axum::Router {
    let public_pem = std::fs::read_to_string("tests/fixtures/test_rsa_public.pem").unwrap();
    let key = DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap();
    let state = Arc::new(AppState {
        db: pool,
        redis: redis::Client::open("redis://127.0.0.1:6379").unwrap(),
        config: test_config(),
        google_verifier: GoogleTokenVerifier::with_seeded_key(TEST_KID, key),
        payment_provider: std::sync::Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        ai_provider: std::sync::Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        text_ai_provider: std::sync::Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        meeting_provider: std::sync::Arc::new(titian_backend_rust::services::meeting_provider::StubMeetingProvider),
        storage: std::sync::Arc::new(titian_backend_rust::services::storage::InMemoryStorage::new()),
        canvas_hub: std::sync::Arc::new(titian_backend_rust::services::canvas_hub::CanvasHub::new()),
        collab_hub: std::sync::Arc::new(titian_backend_rust::services::collab_hub::CollabHub::new()),
    });
    routes::create_router(state)
}

fn sign_test_id_token(sub: &str, email: &str, name: &str) -> String {
    let private_pem = std::fs::read_to_string("tests/fixtures/test_rsa_private.pem").unwrap();
    let key = EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap();
    let now = chrono::Utc::now().timestamp();
    let claims = json!({
        "sub": sub,
        "email": email,
        "email_verified": true,
        "name": name,
        "iss": "https://accounts.google.com",
        "aud": TEST_CLIENT_ID,
        "iat": now,
        "exp": now + 3600,
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KID.to_string());
    jsonwebtoken::encode(&header, &claims, &key).unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[sqlx::test]
async fn google_login_creates_user_and_issues_tokens(pool: PgPool) {
    let app = build_app(pool);
    let id_token = sign_test_id_token("google-sub-1", "new-user@example.com", "New User");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/google/callback")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "id_token": id_token }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body["access_token"].is_string());
    assert!(body["refresh_token"].is_string());
    assert_eq!(body["user"]["email"], "new-user@example.com");
}

#[sqlx::test]
async fn google_login_with_invalid_kid_is_rejected(pool: PgPool) {
    let app = build_app(pool);
    // Sign with the right key but a kid the seeded verifier doesn't
    // recognize — must be rejected, not silently accepted.
    let private_pem = std::fs::read_to_string("tests/fixtures/test_rsa_private.pem").unwrap();
    let key = EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap();
    let now = chrono::Utc::now().timestamp();
    let claims = json!({
        "sub": "x", "email": "x@example.com", "iss": "https://accounts.google.com",
        "aud": TEST_CLIENT_ID, "iat": now, "exp": now + 3600,
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("wrong-kid".to_string());
    let id_token = jsonwebtoken::encode(&header, &claims, &key).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/google/callback")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "id_token": id_token }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(response).await;
    assert_eq!(body["error"], "invalid_token");
}

#[sqlx::test]
async fn refresh_then_me_round_trip(pool: PgPool) {
    let app = build_app(pool);
    let id_token = sign_test_id_token("google-sub-2", "roundtrip@example.com", "Round Trip");

    let login_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/google/callback")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "id_token": id_token }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let login_body = body_json(login_response).await;
    let refresh_token = login_body["refresh_token"].as_str().unwrap().to_string();

    // Refresh
    let refresh_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "refresh_token": refresh_token }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refresh_response.status(), StatusCode::OK);
    let refresh_body = body_json(refresh_response).await;
    let access_token = refresh_body["access_token"].as_str().unwrap().to_string();

    // /users/me with the freshly refreshed access token
    let me_response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/users/me")
                .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me_response.status(), StatusCode::OK);
    let me_body = body_json(me_response).await;
    assert_eq!(me_body["email"], "roundtrip@example.com");
    // Auto-heal: a brand new user gets the default student role on the
    // platform org — mirrors auth_service.ts's P9-002 behavior.
    assert_eq!(me_body["roles"][0]["role"], "student");
}

#[sqlx::test]
async fn me_without_token_is_unauthorized(pool: PgPool) {
    let app = build_app(pool);
    let response = app
        .oneshot(Request::builder().method(Method::GET).uri("/users/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn refresh_with_bogus_token_is_rejected(pool: PgPool) {
    let app = build_app(pool);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "refresh_token": "not-a-real-token" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(response).await;
    assert_eq!(body["error"], "invalid_refresh_token");
}
