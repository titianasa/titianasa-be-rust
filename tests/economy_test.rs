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

use titian_backend_rust::{routes, services::token, state::AppState, Config};

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
        ai_live_chat_model: "~deepseek/deepseek-v4-flash-latest".into(),
    }
}

fn build_app(pool: PgPool) -> axum::Router {
    let state = Arc::new(AppState {
        db: pool,
        redis: redis::Client::open("redis://127.0.0.1:6379").unwrap(),
        config: test_config(),
        google_verifier: titian_backend_rust::services::google_oauth::GoogleTokenVerifier::new(),
        payment_provider: std::sync::Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        ai_provider: std::sync::Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        meeting_provider: std::sync::Arc::new(titian_backend_rust::services::meeting_provider::StubMeetingProvider),
        storage: std::sync::Arc::new(titian_backend_rust::services::storage::InMemoryStorage::new()),
        canvas_hub: std::sync::Arc::new(titian_backend_rust::services::canvas_hub::CanvasHub::new()),
        collab_hub: std::sync::Arc::new(titian_backend_rust::services::collab_hub::CollabHub::new()),
    });
    routes::create_router(state)
}

async fn insert_user(pool: &PgPool, email: &str) -> (Uuid, String) {
    let user_id: Uuid = sqlx::query_scalar!(
        r#"insert into users (google_id, email, name) values ($1, $2, $3) returning id"#,
        format!("google-{email}"),
        email,
        "Test User",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let token = token::issue_access_token(JWT_SECRET, &user_id.to_string(), email, 15).unwrap();
    (user_id, token)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
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

async fn post(app: axum::Router, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
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
async fn credits_default_to_zero_for_a_new_user(pool: PgPool) {
    let (_uid, token) = insert_user(&pool, "credits1@example.com").await;
    let app = build_app(pool);
    let (status, body) = get(app, "/me/credits", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["balance"], 0);
}

#[sqlx::test]
async fn subscribe_grants_allowance_reflected_in_balance(pool: PgPool) {
    let (_uid, token) = insert_user(&pool, "sub1@example.com").await;
    let app = build_app(pool);

    let (status, body) = post(app.clone(), "/subscriptions/subscribe", &token, json!({"tier": "plus"})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["tier"], "plus");
    assert_eq!(body["status"], "active");

    let (status, body) = get(app.clone(), "/me/credits", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["balance"], 100);

    let (status, body) = get(app, "/subscriptions/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["subscribed"], true);
    assert_eq!(body["tier"], "plus");
    assert!(body.get("user_id").is_none());
}

#[sqlx::test]
async fn subscribe_rejects_invalid_tier(pool: PgPool) {
    let (_uid, token) = insert_user(&pool, "sub2@example.com").await;
    let app = build_app(pool);
    let (status, body) = post(app, "/subscriptions/subscribe", &token, json!({"tier": "ultra"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_tier");
}

#[sqlx::test]
async fn subscription_me_is_subscribed_false_when_never_subscribed(pool: PgPool) {
    let (_uid, token) = insert_user(&pool, "sub3@example.com").await;
    let app = build_app(pool);
    let (status, body) = get(app, "/subscriptions/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"subscribed": false}));
}

#[sqlx::test]
async fn watch_ad_creates_a_view(pool: PgPool) {
    let (uid, token) = insert_user(&pool, "ad1@example.com").await;
    let app = build_app(pool.clone());
    let (status, body) = post(app, "/ads/watch", &token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body["view_id"].is_string());

    let count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from ad_views where user_id = $1"#, uid).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
}
