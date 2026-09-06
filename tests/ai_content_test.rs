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
    services::{ai_provider::FakeAIProvider, curriculum_constitution::SECTIONS, token},
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

async fn create_module(app: axum::Router, dev_token: &str, pool: &PgPool, label: &str) -> String {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();
    let (_, module) = send(app, Method::POST, "/modules", dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": format!("MOD-{label}"), "title": "Module"})).await;
    module["id"].as_str().unwrap().to_string()
}

#[sqlx::test]
async fn generate_lesson_creates_draft_lesson_with_ai_provenance(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "genlesson-dev@example.com", "curriculum_developer").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success("# Simple Present\n\nUse it for daily habits.\n\n> I eat breakfast every day.\n")));
    let module_id = create_module(app.clone(), &dev_token, &pool, "GL1").await;

    let (status, body) = send(app.clone(), Method::POST, "/ai/generate-lesson", &dev_token, json!({"module_id": module_id, "content_type": "learn", "topic": "Simple Present"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    assert_eq!(body["status"], "done");
    let item_id = body["item_id"].as_str().unwrap().to_string();

    let (status, lesson) = get(app, &format!("/module-items/{item_id}"), &dev_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lesson["generated_by"], "ai");
    assert_eq!(lesson["status"], "draft");

    let ai_task_status: String = sqlx::query_scalar!(r#"select status from ai_tasks where task_type = 'lesson_generation'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(ai_task_status, "done");
}

#[sqlx::test]
async fn generate_lesson_with_grammar_target_requires_all_11_sections(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "genlesson-grammar-dev@example.com", "curriculum_developer").await;
    // Missing every required section — the model just ignored the instruction.
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success("# Present Perfect\n\nSome content, no numbered sections.\n")));
    let module_id = create_module(app.clone(), &dev_token, &pool, "GL2").await;

    let (status, body) =
        send(app.clone(), Method::POST, "/ai/generate-lesson", &dev_token, json!({"module_id": module_id, "content_type": "learn", "topic": "Present Perfect", "grammar_target": "present_perfect"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "grammar_constitution_incomplete");

    // No item row should have been created — a failed generation never
    // leaves an orphan behind.
    let count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from module_items where module_id = $1"#, Uuid::parse_str(&module_id).unwrap()).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0);

    let ai_task_status: String = sqlx::query_scalar!(r#"select status from ai_tasks where task_type = 'lesson_generation'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(ai_task_status, "failed");

    // Now generate again with every required section present — succeeds.
    let mut alm = String::from("# Present Perfect\n\n");
    for (prefix, title) in SECTIONS {
        alm.push_str(&format!("## {prefix} — {title}\n\nSome content for this section.\n\n"));
    }
    let app2 = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(alm)));
    let (status, body) =
        send(app2, Method::POST, "/ai/generate-lesson", &dev_token, json!({"module_id": module_id, "content_type": "learn", "topic": "Present Perfect", "grammar_target": "present_perfect"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
}

#[sqlx::test]
async fn generate_lesson_provider_failure_returns_422_without_orphan_row(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "genlesson-fail-dev@example.com", "curriculum_developer").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::failure("provider down")));
    let module_id = create_module(app.clone(), &dev_token, &pool, "GL3").await;

    let (status, body) = send(app, Method::POST, "/ai/generate-lesson", &dev_token, json!({"module_id": module_id, "content_type": "learn", "topic": "X"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "ai_output_validation_failed");
}

#[sqlx::test]
async fn generate_questions_creates_drafts_with_ai_provenance(pool: PgPool) {
    let (dev_uid, dev_token) = insert_user_with_role(&pool, "genq-dev@example.com", "curriculum_developer").await;
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('GQ1', 'GQ1') returning id"#).fetch_one(&pool).await.unwrap();
    let items = json!([
        {"data": {"prompt": "I ___ a student.", "options": ["am", "is", "are"]}, "correct_answer": {"index": 0}, "explanation": {"text": "Use \"am\" with \"I\"."}},
        {"data": {"prompt": "She ___ happy.", "options": ["is", "am", "are"]}, "correct_answer": {"index": 0}, "explanation": {"text": "Use \"is\" with \"she\"."}},
    ]);
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(items.to_string())));

    let (_, bank) = send(app.clone(), Method::POST, "/question-banks", &dev_token, json!({"subject_id": subject_id, "name": "Bank"})).await;
    let bank_id = bank["id"].as_str().unwrap().to_string();

    let (status, body) = send(app.clone(), Method::POST, "/ai/generate-questions", &dev_token, json!({"bank_id": bank_id, "question_type": "mcq", "topic": "Verb to be", "count": 2, "difficulty": 0.5})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    let question_ids = body["question_ids"].as_array().unwrap();
    assert_eq!(question_ids.len(), 2);

    let first_id = question_ids[0].as_str().unwrap().to_string();
    let (status, question) = get(app, &format!("/questions/{first_id}"), &dev_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(question["generated_by"], "ai");
    assert_eq!(question["status"], "draft");
    let _ = dev_uid;
}

#[sqlx::test]
async fn generate_questions_count_mismatch_returns_422(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "genq-mismatch-dev@example.com", "curriculum_developer").await;
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('GQ2', 'GQ2') returning id"#).fetch_one(&pool).await.unwrap();
    // Only 1 item returned, but 2 were requested.
    let items = json!([{"data": {"prompt": "I ___ a student.", "options": ["am", "is"]}, "correct_answer": {"index": 0}}]);
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(items.to_string())));

    let (_, bank) = send(app.clone(), Method::POST, "/question-banks", &dev_token, json!({"subject_id": subject_id, "name": "Bank"})).await;
    let bank_id = bank["id"].as_str().unwrap().to_string();

    let (status, body) = send(app, Method::POST, "/ai/generate-questions", &dev_token, json!({"bank_id": bank_id, "question_type": "mcq", "topic": "X", "count": 2, "difficulty": 0.5})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "invalid_ai_output_count");
}

#[sqlx::test]
async fn ocr_to_question_creates_flagged_drafts_for_known_and_unknown_types(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "ocr-dev@example.com", "curriculum_developer").await;
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('OCR1', 'OCR1') returning id"#).fetch_one(&pool).await.unwrap();
    let items = json!([
        {"type": "mcq", "raw_text": "1. I ___ a student. a) am b) is c) are", "data": {"prompt": "I ___ a student.", "options": ["am", "is", "are"]}, "correct_answer": {"index": 0}, "explanation": {"text": "Use \"am\"."}},
        {"type": "unknown", "raw_text": "2. (illegible)", "data": {}, "correct_answer": {}},
    ]);
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(items.to_string())));

    let (_, bank) = send(app.clone(), Method::POST, "/question-banks", &dev_token, json!({"subject_id": subject_id, "name": "Bank"})).await;
    let bank_id = bank["id"].as_str().unwrap().to_string();

    let boundary = "----titianTestBoundary";
    let mut multipart = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"page.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    multipart.extend_from_slice(&[0x89, 0x50, 0x4e, 0x47]);
    multipart.extend_from_slice(b"\r\n");
    multipart.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/assets/upload")
                .header(header::AUTHORIZATION, format!("Bearer {dev_token}"))
                .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
                .body(Body::from(multipart))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let uploaded = body_json(response).await;
    let asset_id = uploaded["id"].as_str().unwrap().to_string();

    let (status, body) = send(app.clone(), Method::POST, "/ai/ocr-to-question", &dev_token, json!({"bank_id": bank_id, "asset_id": asset_id})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    let question_ids = body["question_ids"].as_array().unwrap();
    assert_eq!(question_ids.len(), 2);

    let mcq_id = question_ids[0].as_str().unwrap().to_string();
    let (_, mcq_question) = get(app.clone(), &format!("/questions/{mcq_id}"), &dev_token).await;
    assert_eq!(mcq_question["status"], "draft");
    assert_eq!(mcq_question["qa_report"]["passed"], false);
    let mcq_categories: Vec<String> = mcq_question["qa_report"]["issues"].as_array().unwrap().iter().map(|i| i["category"].as_str().unwrap().to_string()).collect();
    assert_eq!(mcq_categories, vec!["ocr_verification"]); // schema-valid mcq: no uncertain_type flag

    let unknown_id = question_ids[1].as_str().unwrap().to_string();
    let (_, unknown_question) = get(app, &format!("/questions/{unknown_id}"), &dev_token).await;
    assert_eq!(unknown_question["type"], "unknown");
    let unknown_categories: Vec<String> = unknown_question["qa_report"]["issues"].as_array().unwrap().iter().map(|i| i["category"].as_str().unwrap().to_string()).collect();
    assert!(unknown_categories.contains(&"ocr_uncertain_type".to_string()));
}

#[sqlx::test]
async fn speaking_prompt_audio_returns_bytes_for_published_lesson(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "spa-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "spa-rev@example.com", "reviewer").await;
    let (_student_uid, student_token) = insert_user_with_role(&pool, "spa-student@example.com", "student").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success("{}")));
    let module_id = create_module(app.clone(), &dev_token, &pool, "SPA1").await;

    let (status, lesson) = send(
        app.clone(),
        Method::POST,
        &format!("/modules/{module_id}/items"),
        &dev_token,
        json!({"node_type": "item", "title": "Morning Routine", "content_type": "speaking", "format": "markdown", "content": "# Speaking Practice\n\n:::speaking_prompt\ntext: Describe your morning routine.\n:::\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{lesson:?}");
    let item_id = lesson["id"].as_str().unwrap().to_string();

    // Not yet published -> forbidden for a plain student.
    let response = app
        .clone()
        .oneshot(Request::builder().method(Method::GET).uri(format!("/module-items/{item_id}/speaking-prompt-audio")).header(header::AUTHORIZATION, format!("Bearer {student_token}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    send(app.clone(), Method::POST, &format!("/module-items/{item_id}/submit-review"), &dev_token, json!({})).await;
    send(app.clone(), Method::POST, &format!("/module-items/{item_id}/publish"), &reviewer_token, json!({})).await;

    let response = app
        .clone()
        .oneshot(Request::builder().method(Method::GET).uri(format!("/module-items/{item_id}/speaking-prompt-audio")).header(header::AUTHORIZATION, format!("Bearer {student_token}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get(header::CONTENT_TYPE).unwrap(), "audio/mpeg");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), &[0, 1, 2, 3]); // FakeAIProvider's fixed synthesize_speech output
}

#[sqlx::test]
async fn speaking_prompt_audio_404s_when_lesson_has_no_prompt_block(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "spa2-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "spa2-rev@example.com", "reviewer").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success("{}")));
    let module_id = create_module(app.clone(), &dev_token, &pool, "SPA2").await;

    let (_, lesson) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "item", "title": "No Prompt", "content_type": "speaking", "format": "markdown", "content": "# Just text\n"})).await;
    let item_id = lesson["id"].as_str().unwrap().to_string();
    send(app.clone(), Method::POST, &format!("/module-items/{item_id}/submit-review"), &dev_token, json!({})).await;
    send(app.clone(), Method::POST, &format!("/module-items/{item_id}/publish"), &reviewer_token, json!({})).await;

    let (status, body) = get(app, &format!("/module-items/{item_id}/speaking-prompt-audio"), &dev_token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
    assert_eq!(body["error"], "speaking_prompt_not_found");
}
