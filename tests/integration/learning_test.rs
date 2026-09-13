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
        gcp_project_id: "test-gcp-project".into(),
        gcp_region: "us-central1".into(),
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
        text_ai_provider: std::sync::Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        meeting_provider: std::sync::Arc::new(titian_backend_rust::services::meeting_provider::StubMeetingProvider),
        storage: std::sync::Arc::new(titian_backend_rust::services::storage::InMemoryStorage::new()),
        canvas_hub: std::sync::Arc::new(titian_backend_rust::services::canvas_hub::CanvasHub::new()),
        collab_hub: std::sync::Arc::new(titian_backend_rust::services::collab_hub::CollabHub::new()),
    });
    routes::create_router(state)
}

async fn insert_user_with_role(pool: &PgPool, email: &str, role: &str) -> (Uuid, String) {
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
    let token = token::issue_access_token(JWT_SECRET, &user_id.to_string(), email, 15).unwrap();
    (user_id, token)
}

async fn insert_subject(pool: &PgPool, code: &str) -> Uuid {
    sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, code)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_concept(pool: &PgPool, subject_id: Uuid, code: &str, name: &str, parent: Option<Uuid>) -> Uuid {
    sqlx::query_scalar!(
        r#"insert into concepts (subject_id, code, name, type, parent_concept_id) values ($1, $2, $3, 'vocabulary', $4) returning id"#,
        subject_id,
        code,
        name,
        parent,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_mastery(pool: &PgPool, user_id: Uuid, concept_id: Uuid, score: f64, confidence: f64) {
    sqlx::query!(
        r#"insert into masteries (user_id, concept_id, score, confidence, last_reviewed_at) values ($1, $2, $3, $4, now())"#,
        user_id,
        concept_id,
        score,
        confidence,
    )
    .execute(pool)
    .await
    .unwrap();
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

#[sqlx::test]
async fn mastery_insufficient_data_when_no_record(pool: PgPool) {
    let (_uid, student_token) = insert_user_with_role(&pool, "mstudent@example.com", "student").await;
    let subject_id = insert_subject(&pool, "M1").await;
    let concept_id = insert_concept(&pool, subject_id, "c1", "Concept 1", None).await;
    let app = build_app(pool);

    let (status, body) = get(app, &format!("/mastery/{concept_id}"), &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "insufficient_data");
    assert!(body["score"].is_null());
}

#[sqlx::test]
async fn mastery_returns_score_when_confident(pool: PgPool) {
    let (uid, student_token) = insert_user_with_role(&pool, "mstudent2@example.com", "student").await;
    let subject_id = insert_subject(&pool, "M2").await;
    let concept_id = insert_concept(&pool, subject_id, "c1", "Concept 1", None).await;
    insert_mastery(&pool, uid, concept_id, 72.4, 0.8).await;
    let app = build_app(pool);

    let (status, body) = get(app, &format!("/mastery/{concept_id}"), &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["score"], 72);
    assert!(body.get("message").is_none());
}

#[sqlx::test]
async fn mastery_view_forbidden_for_curriculum_developer(pool: PgPool) {
    let (_uid, dev_token) = insert_user_with_role(&pool, "mdev@example.com", "curriculum_developer").await;
    let subject_id = insert_subject(&pool, "M3").await;
    let concept_id = insert_concept(&pool, subject_id, "c1", "Concept 1", None).await;
    let app = build_app(pool);

    let (status, _) = get(app, &format!("/mastery/{concept_id}"), &dev_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn mastery_breakdown_flags_weak_child(pool: PgPool) {
    let (uid, student_token) = insert_user_with_role(&pool, "mstudent3@example.com", "student").await;
    let subject_id = insert_subject(&pool, "M4").await;
    let root = insert_concept(&pool, subject_id, "root", "Root", None).await;
    let child_weak = insert_concept(&pool, subject_id, "child-weak", "Weak Child", Some(root)).await;
    let child_strong = insert_concept(&pool, subject_id, "child-strong", "Strong Child", Some(root)).await;
    insert_mastery(&pool, uid, root, 80.0, 0.9).await;
    insert_mastery(&pool, uid, child_weak, 40.0, 0.9).await;
    insert_mastery(&pool, uid, child_strong, 90.0, 0.9).await;
    let app = build_app(pool);

    let (status, body) = get(app, &format!("/concepts/{root}/mastery-breakdown"), &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["weak"], false);
    let children = body["children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    let weak_child = children.iter().find(|c| c["concept_id"] == child_weak.to_string()).unwrap();
    assert_eq!(weak_child["weak"], true);
    let strong_child = children.iter().find(|c| c["concept_id"] == child_strong.to_string()).unwrap();
    assert_eq!(strong_child["weak"], false);
}

#[sqlx::test]
async fn rescue_status_triggers_after_consecutive_failures(pool: PgPool) {
    let (uid, student_token) = insert_user_with_role(&pool, "mstudent4@example.com", "student").await;
    let subject_id = insert_subject(&pool, "M5").await;
    let concept_id = insert_concept(&pool, subject_id, "c1", "Concept 1", None).await;
    let bank_id: Uuid = sqlx::query_scalar!(r#"insert into question_banks (subject_id, name) values ($1, 'bank') returning id"#, subject_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let question_id: Uuid = sqlx::query_scalar!(
        r#"insert into questions (bank_id, type, data, correct_answer, status) values ($1, 'mcq', '{}', '{}', 'published') returning id"#,
        bank_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(r#"insert into question_concepts (question_id, concept_id) values ($1, $2)"#, question_id, concept_id).execute(&pool).await.unwrap();

    for _ in 0..3 {
        sqlx::query!(
            r#"insert into learning_events (user_id, event_type, entity_type, entity_id, payload) values ($1, 'question_answered', 'question', $2, '{"correct": false}')"#,
            uid,
            question_id,
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    let app = build_app(pool);
    let (status, body) = get(app, &format!("/concepts/{concept_id}/rescue-status"), &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["triggered"], true);
    assert_eq!(body["consecutive_failures"], 3);
}

#[sqlx::test]
async fn review_queue_returns_due_concept_with_suggestions(pool: PgPool) {
    let (uid, student_token) = insert_user_with_role(&pool, "mstudent5@example.com", "student").await;
    let subject_id = insert_subject(&pool, "M6").await;
    let concept_id = insert_concept(&pool, subject_id, "c1", "Due Concept", None).await;
    let bank_id: Uuid = sqlx::query_scalar!(r#"insert into question_banks (subject_id, name) values ($1, 'bank') returning id"#, subject_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let question_id: Uuid = sqlx::query_scalar!(
        r#"insert into questions (bank_id, type, data, correct_answer, status) values ($1, 'mcq', '{}', '{}', 'published') returning id"#,
        bank_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(r#"insert into question_concepts (question_id, concept_id) values ($1, $2)"#, question_id, concept_id).execute(&pool).await.unwrap();
    sqlx::query!(
        r#"insert into frss_schedule (user_id, concept_id, due_at) values ($1, $2, now() - interval '1 hour')"#,
        uid,
        concept_id,
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_app(pool);
    let (status, body) = get(app, "/review-queue", &student_token).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["concept_id"], concept_id.to_string());
    assert_eq!(items[0]["suggested_question_ids"][0], question_id.to_string());
}

#[sqlx::test]
async fn learning_queue_escalates_weak_prerequisite(pool: PgPool) {
    let (uid, student_token) = insert_user_with_role(&pool, "mstudent6@example.com", "student").await;
    let subject_id = insert_subject(&pool, "M7").await;
    let advanced = insert_concept(&pool, subject_id, "advanced", "Advanced Topic", None).await;
    let basic = insert_concept(&pool, subject_id, "basic", "Basic Topic", None).await;
    sqlx::query!(r#"insert into concept_prerequisites (concept_id, prerequisite_concept_id) values ($1, $2)"#, advanced, basic).execute(&pool).await.unwrap();

    // advanced is weak (confident, below threshold); basic has no mastery data at all.
    insert_mastery(&pool, uid, advanced, 40.0, 0.9).await;

    let app = build_app(pool);
    let (status, body) = get(app, "/learning-queue", &student_token).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    let advanced_item = items.iter().find(|i| i["concept_id"] == advanced.to_string()).unwrap();
    assert_eq!(advanced_item["blocked_by_concept_id"], basic.to_string());
    let basic_item = items.iter().find(|i| i["concept_id"] == basic.to_string()).unwrap();
    assert_eq!(basic_item["priority"], "critical");
}

#[sqlx::test]
async fn prerequisite_crud_and_cycle_rejection(pool: PgPool) {
    let (_uid, dev_token) = insert_user_with_role(&pool, "mdev2@example.com", "curriculum_developer").await;
    let subject_id = insert_subject(&pool, "M8").await;
    let a = insert_concept(&pool, subject_id, "a", "A", None).await;
    let b = insert_concept(&pool, subject_id, "b", "B", None).await;
    let app = build_app(pool);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/concepts/{a}/prerequisites"))
                .header(header::AUTHORIZATION, format!("Bearer {dev_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"prerequisite_concept_id": b}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let (status, body) = get(app.clone(), &format!("/concepts/{a}/prerequisites"), &dev_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["concept_id"], b.to_string());

    // Cycle: B cannot require A back.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/concepts/{b}/prerequisites"))
                .header(header::AUTHORIZATION, format!("Bearer {dev_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"prerequisite_concept_id": a}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["error"], "concept_prerequisite_cycle");

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/concepts/{a}/prerequisites/{b}"))
                .header(header::AUTHORIZATION, format!("Bearer {dev_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}
