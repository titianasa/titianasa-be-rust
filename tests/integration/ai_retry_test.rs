// Phase 38 (fondasi AI) — proves the "one retry" behavior added to
// lesson_plan_ai::generate_plan actually clears a one-off provider
// hiccup, and still fails cleanly once the retry budget (1 extra
// attempt) is exhausted. quiz_generation::generate_quiz_group and
// lesson_plan_ai::edit_section share the exact same retry-loop shape
// (same PR) but aren't separately covered here — this is the
// representative case, not full coverage of all three.

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
    services::{ai_provider::FlakyThenSuccessProvider, token},
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
    }
}

fn build_app_with_ai(pool: PgPool, ai_provider: Arc<dyn titian_backend_rust::services::ai_provider::AIProvider>) -> axum::Router {
    let state = Arc::new(AppState {
        db: pool,
        redis: redis::Client::open("redis://127.0.0.1:6379").unwrap(),
        config: test_config(),
        google_verifier: titian_backend_rust::services::google_oauth::GoogleTokenVerifier::new(),
        payment_provider: Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        text_ai_provider: ai_provider.clone(),
        ai_provider,
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

async fn create_module(app: axum::Router, dev_token: &str, pool: &PgPool, label: &str) -> String {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();
    let (_, module) = send(app, Method::POST, "/modules", dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": format!("MOD-{label}"), "title": "Module"})).await;
    module["id"].as_str().unwrap().to_string()
}

/// A minimal reply `parse_generated_plan` can actually turn into a
/// non-empty plan — needs at least one <<<SECTION>>> block (meta as
/// real JSON, per `parse_meta`), or `generate_plan` would reject even a
/// successful attempt as "no sections", muddying what this test checks.
const VALID_PLAN_REPLY: &str = r#"<<<PLAN>>>
{"title": "Pecahan", "topic": "Pecahan", "level": "SD"}
<<<END_PLAN>>>
<<<SECTION>>>
{"title": "Pengantar", "goal": "Paham konsep", "minutes": 5}
<<<CONTENT>>>
Pecahan adalah bagian dari keseluruhan.
<<<END_SECTION>>>
"#;

async fn create_article_item(app: axum::Router, dev_token: &str, module_id: &str) -> String {
    let (status, item) = send(
        app.clone(),
        Method::POST,
        &format!("/modules/{module_id}/items"),
        dev_token,
        json!({"node_type": "item", "title": "Pecahan", "content_type": "article", "format": "markdown", "content": ""}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{item:?}");
    item["id"].as_str().unwrap().to_string()
}

#[sqlx::test]
async fn lesson_plan_generation_retries_once_after_a_transient_provider_failure(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "retry-ok-dev@example.com", "curriculum_developer").await;
    // Fails exactly once, then succeeds — proves the retry loop's
    // second attempt is what saves this call, not that it happened to
    // succeed on the first try.
    let ai = Arc::new(FlakyThenSuccessProvider::new(1, VALID_PLAN_REPLY));
    let app = build_app_with_ai(pool.clone(), ai);
    let module_id = create_module(app.clone(), &dev_token, &pool, "RETRYOK").await;
    let item_id = create_article_item(app.clone(), &dev_token, &module_id).await;

    let (status, body) = send(
        app,
        Method::POST,
        "/ai/generate-lesson-plan",
        &dev_token,
        json!({"item_id": item_id, "topic": "Pecahan", "duration_minutes": 30, "language": "id"}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["lesson_plan"]["sections"].as_array().unwrap().len(), 1);
}

#[sqlx::test]
async fn lesson_plan_generation_fails_once_the_retry_is_also_exhausted(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "retry-fail-dev@example.com", "curriculum_developer").await;
    // Fails twice — more than the retry budget (1 extra attempt) can
    // cover, so this must still surface as a real failure, not hang or
    // silently succeed with garbage.
    let ai = Arc::new(FlakyThenSuccessProvider::new(2, VALID_PLAN_REPLY));
    let app = build_app_with_ai(pool.clone(), ai);
    let module_id = create_module(app.clone(), &dev_token, &pool, "RETRYFAIL").await;
    let item_id = create_article_item(app.clone(), &dev_token, &module_id).await;

    let (status, body) = send(
        app,
        Method::POST,
        "/ai/generate-lesson-plan",
        &dev_token,
        json!({"item_id": item_id, "topic": "Pecahan", "duration_minutes": 30, "language": "id"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "ai_output_validation_failed");
}

#[sqlx::test]
async fn lesson_plan_generation_rejects_a_model_outside_the_allowlist(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "model-not-allowed-dev@example.com", "curriculum_developer").await;
    let ai = Arc::new(FlakyThenSuccessProvider::new(0, VALID_PLAN_REPLY));
    let app = build_app_with_ai(pool.clone(), ai);
    let module_id = create_module(app.clone(), &dev_token, &pool, "MODELBAD").await;
    let item_id = create_article_item(app.clone(), &dev_token, &module_id).await;

    let (status, body) = send(
        app,
        Method::POST,
        "/ai/generate-lesson-plan",
        &dev_token,
        json!({"item_id": item_id, "topic": "Pecahan", "duration_minutes": 30, "language": "id", "model": "some/unlisted-model"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "model_not_allowed");
}
