// Phase 38 (Fase 5b) — generate-batch runs several groups of the same
// item concurrently. quiz_generation::generate_quiz_group used to read
// module_items.quiz_config once, merge in memory, and write the whole
// column back — so two groups finishing around the same time raced: the
// last writer silently discarded whatever the first writer had just
// added, and (independently) both could pick the same "next free
// question number" since merge_into_group discards the model's own
// numbering and renumbers from a number computed before either call's
// write. This proves the fix (a `SELECT ... FOR UPDATE` around the
// read-merge-write) — both groups' new questions survive, and their
// question numbers never collide.

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

/// A draft quiz item with `count` empty `multiple_choice` groups
/// (g0, g1, ...), each in its own section — enough to generate into
/// concurrently without any group depending on another's content.
async fn create_quiz_with_empty_groups(app: axum::Router, dev_token: &str, pool: &PgPool, label: &str, count: usize) -> String {
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

    let sections: Vec<Value> = (0..count).map(|i| json!({"section_id": format!("s{i}"), "title": format!("Bagian {i}")})).collect();
    let groups: Vec<Value> = (0..count)
        .map(|i| json!({"group_id": format!("g{i}"), "type": "multiple_choice", "section_id": format!("s{i}"), "questions": []}))
        .collect();
    let (status, patched) = send(
        app.clone(),
        Method::PATCH,
        &format!("/module-items/{item_id}/quiz-config"),
        dev_token,
        json!({"quiz_config": {"sections": sections, "question_groups": groups}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");
    item_id
}

#[sqlx::test]
async fn concurrent_generation_on_different_groups_loses_neither_write(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "concurrent-gen-dev@example.com", "curriculum_developer").await;
    let app = build_app(pool.clone());
    let item_id = create_quiz_with_empty_groups(app.clone(), &dev_token, &pool, "CONCUR", 2).await;

    let app1 = app.clone();
    let app2 = app.clone();
    let token1 = dev_token.clone();
    let token2 = dev_token.clone();
    let item1 = item_id.clone();
    let item2 = item_id.clone();

    let (r1, r2) = tokio::join!(
        send(app1, Method::POST, "/ai/generate-quiz-group", &token1, json!({"item_id": item1, "group_id": "g0", "mode": "replace", "count": 1})),
        send(app2, Method::POST, "/ai/generate-quiz-group", &token2, json!({"item_id": item2, "group_id": "g1", "mode": "replace", "count": 1})),
    );
    assert_eq!(r1.0, StatusCode::OK, "{:?}", r1.1);
    assert_eq!(r2.0, StatusCode::OK, "{:?}", r2.1);

    let (_, item) = send(app, Method::GET, &format!("/module-items/{item_id}"), &dev_token, json!({})).await;
    let groups = item["quiz_config"]["question_groups"].as_array().unwrap();
    let g0 = groups.iter().find(|g| g["group_id"] == "g0").unwrap();
    let g1 = groups.iter().find(|g| g["group_id"] == "g1").unwrap();

    assert_eq!(g0["questions"].as_array().unwrap().len(), 1, "g0's write must survive g1's concurrent write: {item}");
    assert_eq!(g1["questions"].as_array().unwrap().len(), 1, "g1's write must survive g0's concurrent write: {item}");

    let n0 = g0["questions"][0]["number"].as_i64().unwrap();
    let n1 = g1["questions"][0]["number"].as_i64().unwrap();
    assert_ne!(n0, n1, "both groups must not claim the same question number: g0={n0} g1={n1}");
}
