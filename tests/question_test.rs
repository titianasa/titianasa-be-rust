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
    });
    routes::create_router(state)
}

async fn insert_user_with_role(pool: &PgPool, email: &str, role: &str) -> String {
    let user_id: Uuid = sqlx::query_scalar!(
        r#"insert into users (google_id, email, name) values ($1, $2, $3) returning id"#,
        format!("google-{email}"),
        email,
        "Test User",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let org_id: Uuid = sqlx::query_scalar!(
        r#"insert into organizations (name, slug, type) values ('Test Org', $1, 'school') returning id"#,
        format!("org-{email}"),
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query!(
        r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, $3)"#,
        user_id,
        org_id,
        role,
    )
    .execute(pool)
    .await
    .unwrap();
    token::issue_access_token(JWT_SECRET, &user_id.to_string(), email, 15).unwrap()
}

async fn insert_subject(pool: &PgPool, code: &str) -> Uuid {
    sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, code)
        .fetch_one(pool)
        .await
        .unwrap()
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
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

fn mcq_body() -> Value {
    json!({
        "type": "mcq",
        "difficulty": 0.5,
        "data": {"prompt": "I ___ a student.", "options": ["am", "is", "are"]},
        "correct_answer": {"index": 0},
    })
}

#[sqlx::test]
async fn author_publish_and_read_question_full_flow(pool: PgPool) {
    let dev_token = insert_user_with_role(&pool, "qdev@example.com", "curriculum_developer").await;
    let subject_id = insert_subject(&pool, "QQ1").await;
    let app = build_app(pool.clone());

    let (status, body) = send(app.clone(), Method::POST, "/question-banks", &dev_token, json!({"subject_id": subject_id, "name": "Grammar Bank"})).await;
    assert_eq!(status, StatusCode::CREATED);
    let bank_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = send(app.clone(), Method::POST, &format!("/question-banks/{bank_id}/questions"), &dev_token, mcq_body()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["status"], "draft");
    let question_id = body["id"].as_str().unwrap().to_string();

    // Draft hidden from student.
    let student_token = insert_user_with_role(&pool, "qstudent@example.com", "student").await;
    let (status, body) = get(app.clone(), &format!("/questions/{question_id}/stem"), &student_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "question_not_published");

    // Developer sees full detail including correct_answer.
    let (status, body) = get(app.clone(), &format!("/questions/{question_id}"), &dev_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["correct_answer"]["index"], 0);

    // submit-review -> publish (reviewer role).
    let (status, body) = send(app.clone(), Method::POST, &format!("/questions/{question_id}/submit-review"), &dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "in_review");
    assert_eq!(body["qa_report"]["passed"], true);

    let reviewer_token = insert_user_with_role(&pool, "qreviewer@example.com", "reviewer").await;
    let (status, body) = send(app.clone(), Method::POST, &format!("/questions/{question_id}/publish"), &reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "published");

    // Now student can see the stem, without correct_answer.
    let (status, body) = get(app.clone(), &format!("/questions/{question_id}/stem"), &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("correct_answer").is_none());
    assert_eq!(body["data"]["prompt"], "I ___ a student.");

    // Listing the bank shows it.
    let (status, body) = get(app, &format!("/question-banks/{bank_id}/questions"), &dev_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
}

#[sqlx::test]
async fn invalid_mcq_schema_rejected(pool: PgPool) {
    let dev_token = insert_user_with_role(&pool, "qdev2@example.com", "curriculum_developer").await;
    let subject_id = insert_subject(&pool, "QQ2").await;
    let app = build_app(pool.clone());

    let (_, body) = send(app.clone(), Method::POST, "/question-banks", &dev_token, json!({"subject_id": subject_id, "name": "Bank"})).await;
    let bank_id = body["id"].as_str().unwrap().to_string();

    // Only 1 option — mcq requires at least 2.
    let (status, body) = send(
        app,
        Method::POST,
        &format!("/question-banks/{bank_id}/questions"),
        &dev_token,
        json!({"type": "mcq", "difficulty": 0.5, "data": {"prompt": "x", "options": ["only-one"]}, "correct_answer": {"index": 0}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_question_schema");
}

#[sqlx::test]
async fn create_assessment_bundles_questions(pool: PgPool) {
    let dev_token = insert_user_with_role(&pool, "qdev3@example.com", "curriculum_developer").await;
    let subject_id = insert_subject(&pool, "QQ3").await;
    let app = build_app(pool.clone());

    let (_, body) = send(app.clone(), Method::POST, "/question-banks", &dev_token, json!({"subject_id": subject_id, "name": "Bank"})).await;
    let bank_id = body["id"].as_str().unwrap().to_string();

    let (_, body1) = send(app.clone(), Method::POST, &format!("/question-banks/{bank_id}/questions"), &dev_token, mcq_body()).await;
    let q1 = body1["id"].as_str().unwrap().to_string();
    let (_, body2) = send(app.clone(), Method::POST, &format!("/question-banks/{bank_id}/questions"), &dev_token, mcq_body()).await;
    let q2 = body2["id"].as_str().unwrap().to_string();

    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/assessments",
        &dev_token,
        json!({"type": "unit_test", "title": "Unit 1 Test", "question_ids": [q1, q2]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["question_count"], 2);
    let assessment_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = get(app, &format!("/assessments/{assessment_id}"), &dev_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["question_count"], 2);
    assert_eq!(body["title"], "Unit 1 Test");
}

#[sqlx::test]
async fn create_assessment_rejects_empty_question_ids(pool: PgPool) {
    let dev_token = insert_user_with_role(&pool, "qdev4@example.com", "curriculum_developer").await;
    let app = build_app(pool);

    let (status, body) = send(app, Method::POST, "/assessments", &dev_token, json!({"type": "unit_test", "title": "Empty", "question_ids": []})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_assessment");
}

#[sqlx::test]
async fn student_cannot_create_question_bank(pool: PgPool) {
    let student_token = insert_user_with_role(&pool, "qstudent2@example.com", "student").await;
    let subject_id = insert_subject(&pool, "QQ5").await;
    let app = build_app(pool);

    let (status, _) = send(app, Method::POST, "/question-banks", &student_token, json!({"subject_id": subject_id, "name": "Bank"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
