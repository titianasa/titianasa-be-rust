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
    }
}

fn build_app_with_ai(pool: PgPool, ai_provider: Arc<dyn titian_backend_rust::services::ai_provider::AIProvider>) -> axum::Router {
    let state = Arc::new(AppState {
        db: pool,
        redis: redis::Client::open("redis://127.0.0.1:6379").unwrap(),
        config: test_config(),
        google_verifier: titian_backend_rust::services::google_oauth::GoogleTokenVerifier::new(),
        payment_provider: Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        ai_provider,
        meeting_provider: Arc::new(titian_backend_rust::services::meeting_provider::StubMeetingProvider),
        storage: Arc::new(titian_backend_rust::services::storage::InMemoryStorage::new()),
        canvas_hub: Arc::new(titian_backend_rust::services::canvas_hub::CanvasHub::new()),
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

async fn get(app: axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().method(Method::GET).uri(uri).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[sqlx::test]
async fn evaluate_grammar_charges_credit_and_records_ai_task(pool: PgPool) {
    let (student_uid, student_token) = insert_user_with_role(&pool, "eval-student@example.com", "student").await;
    sqlx::query!(r#"insert into credits (user_id, balance) values ($1, 5)"#, student_uid).execute(&pool).await.unwrap();
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(r#"{"errors":[{"span":[3,5],"issue":"subject_verb_agreement","suggestion":"goes"}]}"#)));

    let (status, body) = send(app.clone(), Method::POST, "/ai/evaluate", &student_token, json!({"task": "grammar_evaluation", "input": {"text": "He go to school."}})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["status"], "done");
    assert_eq!(body["credit_charged"], 1);
    assert_eq!(body["result"]["errors"].as_array().unwrap().len(), 1);

    let (status, balance) = get(app, "/me/credits", &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance["balance"], 4);

    let ai_task_status: String = sqlx::query_scalar!(r#"select status from ai_tasks where user_id = $1 and task_type = 'grammar_evaluation'"#, student_uid).fetch_one(&pool).await.unwrap();
    assert_eq!(ai_task_status, "done");
}

#[sqlx::test]
async fn evaluate_grammar_uses_ad_view_before_credit(pool: PgPool) {
    let (student_uid, student_token) = insert_user_with_role(&pool, "eval-ad-student@example.com", "student").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(r#"{"errors":[]}"#)));

    let (status, watch) = send(app.clone(), Method::POST, "/ads/watch", &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{watch:?}");
    let view_id = Uuid::parse_str(watch["view_id"].as_str().unwrap()).unwrap();

    // No credits row exists at all yet — the ad view must unlock this
    // WITHOUT ever touching the diamond balance.
    let (status, body) = send(app, Method::POST, "/ai/evaluate", &student_token, json!({"task": "grammar_evaluation", "input": {"text": "I am fine."}})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["credit_charged"], 0);

    let consumed: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar!(r#"select consumed_at from ad_views where id = $1"#, view_id).fetch_one(&pool).await.unwrap();
    assert!(consumed.is_some());
    let has_credits_row: Option<Uuid> = sqlx::query_scalar!(r#"select user_id from credits where user_id = $1"#, student_uid).fetch_optional(&pool).await.unwrap();
    assert!(has_credits_row.is_none());
}

#[sqlx::test]
async fn evaluate_insufficient_credit_without_ad_view(pool: PgPool) {
    let (_student_uid, student_token) = insert_user_with_role(&pool, "eval-poor-student@example.com", "student").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(r#"{"errors":[]}"#)));

    let (status, body) = send(app, Method::POST, "/ai/evaluate", &student_token, json!({"task": "grammar_evaluation", "input": {"text": "text"}})).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body:?}");
    assert_eq!(body["error"], "insufficient_credit");
    assert_eq!(body["required"], 1);
    assert_eq!(body["balance"], 0);
}

#[sqlx::test]
async fn evaluate_unsupported_task_type_rejected(pool: PgPool) {
    let (_student_uid, student_token) = insert_user_with_role(&pool, "eval-unsupported@example.com", "student").await;
    let app = build_app_with_ai(pool, Arc::new(FakeAIProvider::success("{}")));

    let (status, body) = send(app, Method::POST, "/ai/evaluate", &student_token, json!({"task": "essay_scoring", "input": {"text": "x"}})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "unsupported_ai_task");
}

#[sqlx::test]
async fn evaluate_provider_failure_records_failed_ai_task_and_charges_nothing(pool: PgPool) {
    let (student_uid, student_token) = insert_user_with_role(&pool, "eval-fail-student@example.com", "student").await;
    sqlx::query!(r#"insert into credits (user_id, balance) values ($1, 5)"#, student_uid).execute(&pool).await.unwrap();
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::failure("provider is down")));

    let (status, body) = send(app, Method::POST, "/ai/evaluate", &student_token, json!({"task": "grammar_evaluation", "input": {"text": "text"}})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "ai_output_validation_failed");
    assert!(body["detail"].is_null());

    let ai_task_status: String = sqlx::query_scalar!(r#"select status from ai_tasks where user_id = $1 and task_type = 'grammar_evaluation'"#, student_uid).fetch_one(&pool).await.unwrap();
    assert_eq!(ai_task_status, "failed");
    let balance: i64 = sqlx::query_scalar!(r#"select balance from credits where user_id = $1"#, student_uid).fetch_one(&pool).await.unwrap();
    assert_eq!(balance, 5);
}
