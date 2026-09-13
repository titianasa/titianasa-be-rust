// Phase 38 (Fase 5b) — POST /ai/quiz/generate-batch fills every
// still-empty group in one item, reporting each group's outcome
// separately so one bad group doesn't sink the whole run.

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
      "stem": "Ibu kota Indonesia adalah ...",
      "choices": [
        {"label": "A", "text": "Bandung"},
        {"label": "B", "text": "Jakarta"},
        {"label": "C", "text": "Surabaya"},
        {"label": "D", "text": "Medan"}
      ],
      "answer": "B",
      "explanation": "Jakarta adalah ibu kota Indonesia sejak 1945."
    }
  ]
}"#;

fn build_app(pool: PgPool, ai_provider: Arc<dyn titian_backend_rust::services::ai_provider::AIProvider>) -> axum::Router {
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

/// A draft quiz with 3 groups: two empty `multiple_choice` groups (worth
/// generating) and one `h5p` group (hand-authored, must be skipped by
/// the batch rather than errored on).
async fn create_quiz_for_batch(app: axum::Router, dev_token: &str, pool: &PgPool, label: &str) -> String {
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
        json!({"quiz_config": {
            "sections": [{"section_id": "s1", "title": "Bagian 1"}],
            "question_groups": [
                {"group_id": "g0", "type": "multiple_choice", "section_id": "s1", "questions": []},
                {"group_id": "g1", "type": "multiple_choice", "section_id": "s1", "questions": []},
                {"group_id": "g2", "type": "h5p", "section_id": "s1", "questions": []},
            ],
        }}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");
    item_id
}

#[sqlx::test]
async fn batch_fills_every_empty_generatable_group_and_skips_hand_authored_ones(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "batch-dev@example.com", "curriculum_developer").await;
    let ai = Arc::new(FakeAIProvider::success(VALID_MC_REPLY));
    let app = build_app(pool.clone(), ai);
    let item_id = create_quiz_for_batch(app.clone(), &dev_token, &pool, "BATCH").await;

    let (status, body) = send(app.clone(), Method::POST, "/ai/quiz/generate-batch", &dev_token, json!({"item_id": item_id, "count": 2})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "the h5p group must not appear in the batch at all: {results:?}");
    for r in results {
        assert_eq!(r["status"], "done", "{r:?}");
        // The fake provider always returns the same fixed 1-question
        // reply regardless of the requested count — this checks the
        // merge actually happened, not that count=2 was honored.
        assert_eq!(r["question_count"], 1);
    }

    let (_, item) = send(app, Method::GET, &format!("/module-items/{item_id}"), &dev_token, json!({})).await;
    let groups = item["quiz_config"]["question_groups"].as_array().unwrap();
    let g0 = groups.iter().find(|g| g["group_id"] == "g0").unwrap();
    let g1 = groups.iter().find(|g| g["group_id"] == "g1").unwrap();
    let g2 = groups.iter().find(|g| g["group_id"] == "g2").unwrap();
    assert_eq!(g0["questions"].as_array().unwrap().len(), 1);
    assert_eq!(g1["questions"].as_array().unwrap().len(), 1);
    assert_eq!(g2["questions"].as_array().unwrap().len(), 0, "the h5p group is untouched");
    // Fase 5d — a batch fill happens without the author looking at any
    // one group, so every filled group comes back marked draft.
    assert_eq!(g0["ai_meta"]["draft"], true, "{g0:?}");
    assert_eq!(g1["ai_meta"]["draft"], true, "{g1:?}");
}

#[sqlx::test]
async fn batch_reports_one_groups_failure_without_sinking_the_others(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "batch-fail-dev@example.com", "curriculum_developer").await;
    // Always fails to parse — every group in the batch fails, but each
    // must be reported individually (not a single opaque 500/422).
    let ai = Arc::new(FakeAIProvider::success("not json"));
    let app = build_app(pool.clone(), ai);
    let item_id = create_quiz_for_batch(app.clone(), &dev_token, &pool, "BATCHFAIL").await;

    let (status, body) = send(app, Method::POST, "/ai/quiz/generate-batch", &dev_token, json!({"item_id": item_id})).await;
    assert_eq!(status, StatusCode::OK, "the batch call itself must succeed even though every group failed: {body:?}");

    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for r in results {
        assert_eq!(r["status"], "failed", "{r:?}");
        // The real reason, not a bare error code — GCP migration made
        // AiOutputValidationFailed carry the actual parse/provider error.
        let error = r["error"].as_str().unwrap();
        assert!(error.contains("JSON"), "expected a JSON-parse-failure message, got: {error}");
    }
}

#[sqlx::test]
async fn batch_with_nothing_left_to_generate_returns_an_empty_result_list(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "batch-empty-dev@example.com", "curriculum_developer").await;
    let ai = Arc::new(FakeAIProvider::success(VALID_MC_REPLY));
    let app = build_app(pool.clone(), ai);

    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, "SUBJ-BATCHNONE").fetch_one(&pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": "MOD-BATCHNONE", "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();
    let (_, item) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "item", "title": "Quiz", "content_type": "quiz"})).await;
    let item_id = item["id"].as_str().unwrap().to_string();
    // A quiz_config with zero groups — distinct from no quiz_config at
    // all (that stays a 404, same as generate-quiz-group on a
    // nonexistent config).
    let (status, patched) = send(app.clone(), Method::PATCH, &format!("/module-items/{item_id}/quiz-config"), &dev_token, json!({"quiz_config": {"sections": [], "question_groups": []}})).await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");

    let (status, body) = send(app, Method::POST, "/ai/quiz/generate-batch", &dev_token, json!({"item_id": item_id})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["results"].as_array().unwrap().len(), 0);
}
