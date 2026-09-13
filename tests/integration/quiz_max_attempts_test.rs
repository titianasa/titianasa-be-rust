// Phase 38 (Fase 1) — "Maks. Percobaan": a learner may only START a
// quiz `quiz_config.max_attempts` times. No prior integration test
// exercised the Phase 37 quiz-config/attempt flow over real HTTP at
// all (only --lib unit tests), so this also doubles as the first one.

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

/// Author -> quiz_config PATCH -> submit-review -> publish, so the item
/// is a real `content_type = "quiz"`, published item with an
/// author-set `max_attempts`. 0 question_groups is deliberate — this
/// only exercises the attempt-creation gate, not grading.
async fn create_published_quiz(app: axum::Router, dev_token: &str, reviewer_token: &str, pool: &PgPool, label: &str, max_attempts: Option<i64>) -> String {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": format!("MOD-{label}"), "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();
    let (status, item) = send(
        app.clone(),
        Method::POST,
        &format!("/modules/{module_id}/items"),
        dev_token,
        json!({"node_type": "item", "title": format!("Quiz {label}"), "content_type": "quiz"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{item:?}");
    let item_id = item["id"].as_str().unwrap().to_string();

    let (status, patched) = send(
        app.clone(),
        Method::PATCH,
        &format!("/module-items/{item_id}/quiz-config"),
        dev_token,
        json!({"quiz_config": {"question_groups": [], "max_attempts": max_attempts}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");

    let (status, _) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/submit-review"), dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, published) = send(app, Method::POST, &format!("/module-items/{item_id}/publish"), reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{published:?}");
    item_id
}

#[sqlx::test]
async fn a_learner_may_not_start_beyond_max_attempts(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "maxatt-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "maxatt-rev@example.com", "reviewer").await;
    let (_student_uid, student_token) = insert_user_with_role(&pool, "maxatt-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let item_id = create_published_quiz(app.clone(), &dev_token, &reviewer_token, &pool, "MAXATT", Some(2)).await;

    // Attempt 1: starts, then submitted so a 2nd may start (only one
    // in_progress attempt is allowed at a time regardless of the cap).
    let (status, attempt1) = send(app.clone(), Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt1:?}");
    let attempt1_id = attempt1["attempt_id"].as_str().unwrap().to_string();
    let (status, submitted) = send(app.clone(), Method::POST, &format!("/attempts/{attempt1_id}/submit"), &student_token, json!({"quiz_answers": {}})).await;
    assert_eq!(status, StatusCode::OK, "{submitted:?}");

    // Attempt 2 of 2 -> still allowed.
    let (status, attempt2) = send(app.clone(), Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt2:?}");
    let attempt2_id = attempt2["attempt_id"].as_str().unwrap().to_string();
    let (status, submitted) = send(app.clone(), Method::POST, &format!("/attempts/{attempt2_id}/submit"), &student_token, json!({"quiz_answers": {}})).await;
    assert_eq!(status, StatusCode::OK, "{submitted:?}");

    // Attempt 3 -> the cap of 2 is already spent -> rejected.
    let (status, body) = send(app, Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "max_attempts_reached");
}

#[sqlx::test]
async fn no_max_attempts_set_keeps_todays_unlimited_behavior(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "noattcap-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "noattcap-rev@example.com", "reviewer").await;
    let (_student_uid, student_token) = insert_user_with_role(&pool, "noattcap-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let item_id = create_published_quiz(app.clone(), &dev_token, &reviewer_token, &pool, "NOATTCAP", None).await;

    for _ in 0..3 {
        let (status, attempt) = send(app.clone(), Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
        assert_eq!(status, StatusCode::CREATED, "{attempt:?}");
        let attempt_id = attempt["attempt_id"].as_str().unwrap().to_string();
        let (status, submitted) = send(app.clone(), Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"quiz_answers": {}})).await;
        assert_eq!(status, StatusCode::OK, "{submitted:?}");
    }
}
