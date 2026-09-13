// Phase 38 (Fase 5b) — "Tempel & Parse": pasting text the author already
// has (a PDF's copied text, an old worksheet) should extract those
// questions rather than invent new ones, the same idea generate-quiz-group
// already offers for an attached image, just for `raw_text` instead of
// `asset_id`. Proven over real HTTP so the wiring (request -> blueprint ->
// build_prompt) is covered end to end, not just the prompt-string unit
// tests in quiz_generation.rs.

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
    }
}

const VALID_MC_REPLY: &str = r#"{
  "questions": [
    {
      "number": 1,
      "stem": "Apa ibu kota Perancis?",
      "choices": [
        {"label": "A", "text": "Lyon"},
        {"label": "B", "text": "Paris"},
        {"label": "C", "text": "Marseille"},
        {"label": "D", "text": "Nice"}
      ],
      "answer": "B",
      "explanation": "Paris adalah ibu kota Perancis."
    }
  ]
}"#;

fn build_app(pool: PgPool) -> axum::Router {
    let state = Arc::new(AppState {
        db: pool,
        redis: redis::Client::open("redis://127.0.0.1:6379").unwrap(),
        config: test_config(),
        google_verifier: titian_backend_rust::services::google_oauth::GoogleTokenVerifier::new(),
        payment_provider: Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        ai_provider: Arc::new(FakeAIProvider::success(VALID_MC_REPLY)),
        text_ai_provider: Arc::new(FakeAIProvider::success(VALID_MC_REPLY)),
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
async fn pasted_text_extracts_into_an_empty_group(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "parseraw-dev@example.com", "curriculum_developer").await;
    let app = build_app(pool.clone());

    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, "SUBJ-PARSERAW").fetch_one(&pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": "MOD-PARSERAW", "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();
    let (_, item) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "item", "title": "Quiz", "content_type": "quiz"})).await;
    let item_id = item["id"].as_str().unwrap().to_string();
    let (status, patched) = send(
        app.clone(),
        Method::PATCH,
        &format!("/module-items/{item_id}/quiz-config"),
        &dev_token,
        json!({"quiz_config": {"sections": [{"section_id": "s1", "title": "Bagian 1"}], "question_groups": [{"group_id": "g0", "type": "multiple_choice", "section_id": "s1", "questions": []}]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");

    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/ai/generate-quiz-group",
        &dev_token,
        json!({
            "item_id": item_id,
            "group_id": "g0",
            "mode": "replace",
            "raw_text": "1. Apa ibu kota Perancis?\nA. Lyon\nB. Paris\nC. Marseille\nD. Nice\nJawaban: B",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["question_count"], 1);

    let (_, item) = send(app, Method::GET, &format!("/module-items/{item_id}"), &dev_token, json!({})).await;
    let g0 = item["quiz_config"]["question_groups"][0].clone();
    assert_eq!(g0["questions"][0]["stem"], "Apa ibu kota Perancis?");
    assert_eq!(g0["questions"][0]["answer"], "B");
}
