// P39-002 (ADR-0013 L1) — publishing an item freezes a version, and a
// quiz attempt records which version it was actually scored against.

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
    models::auth::AuthContext,
    routes,
    services::{ai_provider::FakeAIProvider, module_item_version, token},
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

/// Same shape as `quiz_max_attempts_test.rs`'s helper, but stops one
/// step short of publish so each test controls that moment itself.
async fn create_in_review_quiz(app: axum::Router, dev_token: &str, pool: &PgPool, label: &str, quiz_config: Value) -> String {
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

    let (status, patched) = send(app.clone(), Method::PATCH, &format!("/module-items/{item_id}/quiz-config"), dev_token, json!({"quiz_config": quiz_config})).await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");

    let (status, _) = send(app, Method::POST, &format!("/module-items/{item_id}/submit-review"), dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    item_id
}

fn one_mcq_group() -> Value {
    json!({"question_groups": [{
        "group_id": "g1", "type": "multiple_choice",
        "questions": [{"number": 1, "stem": "1 + 1 = ?", "choices": [{"label": "A", "text": "1"}, {"label": "B", "text": "2"}], "answer": "B"}],
    }]})
}

#[sqlx::test]
async fn publishing_freezes_version_1_with_the_content_that_was_actually_published(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "ver1-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "ver1-rev@example.com", "reviewer").await;
    let app = build_app(pool.clone());

    let item_id = create_in_review_quiz(app.clone(), &dev_token, &pool, "VER1", one_mcq_group()).await;

    // Before publish: no version exists yet.
    let (status, before) = send(app.clone(), Method::GET, &format!("/module-items/{item_id}/versions"), &dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before.as_array().unwrap().len(), 0);

    let (status, published) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/publish"), &reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{published:?}");

    let (status, versions) = send(app.clone(), Method::GET, &format!("/module-items/{item_id}/versions"), &dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let versions = versions.as_array().unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0]["version"], json!(1));
    assert_eq!(versions[0]["created_via"], json!("human"));
    assert!(versions[0]["superseded_at"].is_null(), "the only version is still current");

    let (status, detail) = send(app, Method::GET, &format!("/module-items/{item_id}/versions/1"), &dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{detail:?}");
    assert_eq!(detail["quiz_config"]["question_groups"][0]["questions"][0]["stem"], json!("1 + 1 = ?"), "the frozen snapshot has to be the ACTUAL published content, not a placeholder");
}

#[sqlx::test]
async fn an_item_with_no_lesson_plan_or_quiz_config_freezes_nothing(pool: PgPool) {
    // A plain legacy article (content lives only in content_blocks) must
    // not get a meaningless empty version 1 just for existing.
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "verplain-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "verplain-rev@example.com", "reviewer").await;
    let app = build_app(pool.clone());

    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('SUBJ-VERPLAIN', 'SUBJ-VERPLAIN') returning id"#).fetch_one(&pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": "MOD-VERPLAIN", "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();
    let (status, item) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "item", "title": "Plain Article", "content_type": "article"})).await;
    assert_eq!(status, StatusCode::CREATED, "{item:?}");
    let item_id = item["id"].as_str().unwrap().to_string();

    let (status, _) = send(app.clone(), Method::PATCH, &format!("/module-items/{item_id}"), &dev_token, json!({"content": "# Hello\nplain ALM", "format": "markdown"})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/submit-review"), &dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, published) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/publish"), &reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{published:?}");

    let (status, versions) = send(app, Method::GET, &format!("/module-items/{item_id}/versions"), &dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(versions.as_array().unwrap().len(), 0, "a plain content_blocks-only article has nothing in the new shape to freeze");
}

/// There is no "unpublish"/re-draft flow yet (ADR-0008's own MVP-scope
/// note: a published item simply refuses further edits), so a second
/// REAL publish isn't reachable through the product today. This tests
/// `module_item_version::freeze` directly instead — the mechanism a
/// future re-publish flow (or Phase 42's AI proposals) will call.
#[sqlx::test]
async fn a_second_freeze_supersedes_the_first_and_numbers_sequentially(pool: PgPool) {
    let (dev_uid, dev_token) = insert_user_with_role(&pool, "ver2-dev@example.com", "curriculum_developer").await;
    let app = build_app(pool.clone());

    let item_id_str = create_in_review_quiz(app.clone(), &dev_token, &pool, "VER2", one_mcq_group()).await;
    let item_id = Uuid::parse_str(&item_id_str).unwrap();
    // The item is only `in_review` (create_in_review_quiz stops before
    // publish) — freeze doesn't care about status, only module_item::
    // publish does, so this is a direct, honest test of the mechanism.
    let ctx = AuthContext { user_id: dev_uid, organization_id: None, role: Some("curriculum_developer".to_string()) };

    let v1 = module_item_version::freeze(&pool, &ctx, item_id, None, Some(&one_mcq_group()), "human", None).await.unwrap();
    assert_eq!(v1, 1);

    let v2_config = json!({"question_groups": [{"group_id": "g1", "type": "multiple_choice", "questions": [{"number": 1, "stem": "2 + 2 = ?", "answer": "4"}]}]});
    let v2 = module_item_version::freeze(&pool, &ctx, item_id, None, Some(&v2_config), "ai_generation", Some("diperbaiki oleh AI")).await.unwrap();
    assert_eq!(v2, 2);

    let versions = module_item_version::list(&pool, &ctx, item_id).await.unwrap();
    assert_eq!(versions.len(), 2);
    // Newest first.
    assert_eq!(versions[0].version, 2);
    assert_eq!(versions[0].created_via, "ai_generation");
    assert!(versions[0].superseded_at.is_none(), "the current version is not superseded");
    assert_eq!(versions[1].version, 1);
    assert!(versions[1].superseded_at.is_some(), "the old version must be marked superseded once a new one exists");
    assert_ne!(versions[0].content_hash, versions[1].content_hash, "different content must hash differently");

    let detail = module_item_version::get(&pool, &ctx, item_id, 1).await.unwrap();
    assert_eq!(detail.quiz_config.unwrap()["question_groups"][0]["questions"][0]["stem"], json!("1 + 1 = ?"), "version 1's snapshot must still read back exactly as it was frozen, unaffected by version 2");

    let current_version: Option<i32> = sqlx::query_scalar!(r#"select current_version from module_items where id = $1"#, item_id).fetch_one(&pool).await.unwrap();
    assert_eq!(current_version, Some(2));
}

#[sqlx::test]
async fn an_attempt_records_the_content_version_that_was_live_when_it_was_scored(pool: PgPool) {
    let (dev_uid, dev_token) = insert_user_with_role(&pool, "verattempt-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "verattempt-rev@example.com", "reviewer").await;
    let (_student_uid, student_token) = insert_user_with_role(&pool, "verattempt-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let item_id_str = create_in_review_quiz(app.clone(), &dev_token, &pool, "VERATTEMPT", one_mcq_group()).await;
    let item_id = Uuid::parse_str(&item_id_str).unwrap();
    let (status, _) = send(app.clone(), Method::POST, &format!("/module-items/{item_id_str}/publish"), &reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);

    // Attempt #1 — scored while current_version == 1.
    let (status, attempt1) = send(app.clone(), Method::POST, &format!("/lessons/{item_id_str}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt1:?}");
    let attempt1_id = attempt1["attempt_id"].as_str().unwrap().to_string();
    let (status, submitted1) = send(app.clone(), Method::POST, &format!("/attempts/{attempt1_id}/submit"), &student_token, json!({"quiz_answers": {"1": "B"}})).await;
    assert_eq!(status, StatusCode::OK, "{submitted1:?}");

    let v1: Option<i32> = sqlx::query_scalar!(r#"select content_version from attempts where id = $1"#, Uuid::parse_str(&attempt1_id).unwrap()).fetch_one(&pool).await.unwrap();
    assert_eq!(v1, Some(1));

    // Freeze a hypothetical version 2 directly (no product flow reaches
    // this yet — see the comment on the freeze-mechanism test above).
    let ctx = AuthContext { user_id: dev_uid, organization_id: None, role: Some("curriculum_developer".to_string()) };
    let v2_config = json!({"question_groups": [{"group_id": "g1", "type": "multiple_choice", "questions": [{"number": 1, "stem": "3 + 3 = ?", "answer": "6"}]}]});
    module_item_version::freeze(&pool, &ctx, item_id, None, Some(&v2_config), "human", None).await.unwrap();
    sqlx::query!(r#"update module_items set quiz_config = $2 where id = $1"#, item_id, v2_config).execute(&pool).await.unwrap();

    // Attempt #2 — scored while current_version == 2.
    let (status, attempt2) = send(app.clone(), Method::POST, &format!("/lessons/{item_id_str}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt2:?}");
    let attempt2_id = attempt2["attempt_id"].as_str().unwrap().to_string();
    let (status, submitted2) = send(app, Method::POST, &format!("/attempts/{attempt2_id}/submit"), &student_token, json!({"quiz_answers": {"1": "6"}})).await;
    assert_eq!(status, StatusCode::OK, "{submitted2:?}");

    let v2: Option<i32> = sqlx::query_scalar!(r#"select content_version from attempts where id = $1"#, Uuid::parse_str(&attempt2_id).unwrap()).fetch_one(&pool).await.unwrap();
    assert_eq!(v2, Some(2), "the second attempt must record the version that was live when IT was scored, not the first attempt's");
}
