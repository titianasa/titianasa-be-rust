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
        ai_provider: Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        text_ai_provider: Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        meeting_provider: Arc::new(titian_backend_rust::services::meeting_provider::StubMeetingProvider),
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
    sqlx::query!(r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, $3)"#, user_id, org_id, role)
        .execute(pool)
        .await
        .unwrap();
    let token = token::issue_access_token(JWT_SECRET, &user_id.to_string(), email, 15).unwrap();
    (user_id, token)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn send(app: axum::Router, method: Method, uri: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app.oneshot(builder.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn get(app: axum::Router, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app.oneshot(builder.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn create_group_cohort(app: axum::Router, tutor_token: &str, capacity: i64) -> (String, String) {
    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/tutors/me/products",
        Some(tutor_token),
        json!({"type": "group", "title": "Group Class", "price_idr": 100000, "capacity": capacity}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "product create failed: {body:?}");
    let product_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = send(app, Method::POST, &format!("/products/{product_id}/cohorts"), Some(tutor_token), json!({"name": "Batch 1"})).await;
    assert_eq!(status, StatusCode::CREATED, "cohort create failed: {body:?}");
    (product_id, body["id"].as_str().unwrap().to_string())
}

#[sqlx::test]
async fn product_lifecycle_visibility_and_publish(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor1@example.com", "tutor").await;
    let (_uid2, other_token) = insert_user_with_role(&pool, "other1@example.com", "student").await;
    let app = build_app(pool);

    let (status, body) =
        send(app.clone(), Method::POST, "/tutors/me/products", Some(&tutor_token), json!({"type": "private", "title": "1-on-1", "price_idr": 50000})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["status"], "draft");
    let product_id = body["id"].as_str().unwrap().to_string();

    // Draft hidden from a non-owner (404, not 403).
    let (status, body) = get(app.clone(), &format!("/products/{product_id}"), Some(&other_token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "learning_product_not_found");

    // Not-owner publish attempt -> 404 too (no matrix role bypass).
    let (status, _) = send(app.clone(), Method::POST, &format!("/products/{product_id}/publish"), Some(&other_token), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(app.clone(), Method::POST, &format!("/products/{product_id}/publish"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "published");

    // Now visible to anyone.
    let (status, _) = get(app, &format!("/products/{product_id}"), Some(&other_token)).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn group_product_requires_capacity_and_private_rejects_it(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor2@example.com", "tutor").await;
    let app = build_app(pool);

    let (status, body) =
        send(app.clone(), Method::POST, "/tutors/me/products", Some(&tutor_token), json!({"type": "group", "title": "X", "price_idr": 1000})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_capacity");

    let (status, body) = send(
        app,
        Method::POST,
        "/tutors/me/products",
        Some(&tutor_token),
        json!({"type": "private", "title": "X", "price_idr": 1000, "capacity": 5}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_capacity");
}

#[sqlx::test]
async fn enroll_self_is_idempotent_and_respects_capacity(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor3@example.com", "tutor").await;
    let (_uid2, student1) = insert_user_with_role(&pool, "student1@example.com", "student").await;
    let (_uid3, student2) = insert_user_with_role(&pool, "student2@example.com", "student").await;
    let app = build_app(pool);
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 1).await;

    let (status, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student1), json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["status"], "pending");

    // Repeat call is idempotent — same row back, still 201.
    let (status, body2) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student1), json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body2["id"], body["id"]);

    // A 2nd distinct student hits capacity=1 already filled by student1's pending enrollment.
    let (status, body) = send(app, Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student2), json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "cohort_full");
}

#[sqlx::test]
async fn checkout_webhook_activates_enrollment_and_pays_tutor(pool: PgPool) {
    let (tutor_uid, tutor_token) = insert_user_with_role(&pool, "tutor4@example.com", "tutor").await;
    let (_uid, student_token) = insert_user_with_role(&pool, "student3@example.com", "student").await;
    let app = build_app(pool.clone());
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;

    let (_, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;
    let enrollment_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = send(app.clone(), Method::POST, &format!("/enrollments/{enrollment_id}/checkout"), Some(&student_token), json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["status"], "pending");
    assert_eq!(body["amount_idr"], 100000);
    assert!(body["qris_payload"].as_str().unwrap().contains("STUB-QRIS"));
    let payment_id = body["payment_id"].as_str().unwrap().to_string();

    // A 2nd checkout on the same enrollment is a conflict.
    let (status, _) = send(app.clone(), Method::POST, &format!("/enrollments/{enrollment_id}/checkout"), Some(&student_token), json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Webhook — public, no token.
    let (status, body) = send(app.clone(), Method::POST, &format!("/payments/{payment_id}/webhook"), None, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "paid");

    // Idempotent — calling again is a no-op, still paid.
    let (status, body2) = send(app.clone(), Method::POST, &format!("/payments/{payment_id}/webhook"), None, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body2["status"], "paid");
    assert_eq!(body2["id"], body["id"]);

    // Enrollment flipped to active.
    let (_, my_enrollments) = get(app.clone(), "/me/enrollments", Some(&student_token)).await;
    assert_eq!(my_enrollments["items"][0]["status"], "active");
    assert_eq!(my_enrollments["items"][0]["order_status"], "paid");

    // Tutor's wallet reflects the 70% share (100000 * 0.7 = 70000),
    // even after 2 webhook deliveries (idempotent, no double-pay).
    let tutor_ctx_token = tutor_token.clone();
    let (_, wallet_body) = get(app, "/tutors/me/wallet", Some(&tutor_ctx_token)).await;
    assert_eq!(wallet_body["balance_idr"], 70000);
    let _ = tutor_uid;
}

#[sqlx::test]
async fn cancel_before_payment_marks_order_failed_no_refund(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor5@example.com", "tutor").await;
    let (_uid2, student_token) = insert_user_with_role(&pool, "student4@example.com", "student").await;
    let app = build_app(pool.clone());
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;

    let (_, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;
    let enrollment_id = body["id"].as_str().unwrap().to_string();
    send(app.clone(), Method::POST, &format!("/enrollments/{enrollment_id}/checkout"), Some(&student_token), json!({})).await;

    let (status, body) = send(app.clone(), Method::POST, &format!("/enrollments/{enrollment_id}/cancel"), Some(&student_token), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "cancelled");
    assert_eq!(body["refund_amount_idr"], 0);

    let order_status: String = sqlx::query_scalar!(r#"select status from orders where enrollment_id = $1"#, Uuid::parse_str(&enrollment_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(order_status, "failed");
}

#[sqlx::test]
async fn cancel_after_payment_far_from_start_gives_full_refund(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor6@example.com", "tutor").await;
    let (_uid2, student_token) = insert_user_with_role(&pool, "student5@example.com", "student").await;
    let app = build_app(pool.clone());

    let (_, body) = send(
        app.clone(),
        Method::POST,
        "/tutors/me/products",
        Some(&tutor_token),
        json!({"type": "group", "title": "G", "price_idr": 100000, "capacity": 5}),
    )
    .await;
    let product_id = body["id"].as_str().unwrap().to_string();
    let far_future = (chrono::Utc::now() + chrono::Duration::days(10)).to_rfc3339();
    let (_, body) =
        send(app.clone(), Method::POST, &format!("/products/{product_id}/cohorts"), Some(&tutor_token), json!({"name": "B", "starts_at": far_future})).await;
    let cohort_id = body["id"].as_str().unwrap().to_string();

    let (_, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;
    let enrollment_id = body["id"].as_str().unwrap().to_string();
    let (_, checkout_body) = send(app.clone(), Method::POST, &format!("/enrollments/{enrollment_id}/checkout"), Some(&student_token), json!({})).await;
    let payment_id = checkout_body["payment_id"].as_str().unwrap().to_string();
    send(app.clone(), Method::POST, &format!("/payments/{payment_id}/webhook"), None, json!({})).await;

    let (status, body) = send(app, Method::POST, &format!("/enrollments/{enrollment_id}/cancel"), Some(&student_token), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["refund_amount_idr"], 100000);
}

#[sqlx::test]
async fn certificate_requires_completed_enrollment_and_verify_is_public(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor7@example.com", "tutor").await;
    let (_uid2, student_token) = insert_user_with_role(&pool, "student6@example.com", "student").await;
    let app = build_app(pool.clone());
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;

    let (_, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;
    let enrollment_id = body["id"].as_str().unwrap().to_string();

    // Not completed yet -> reject.
    let (status, body) =
        send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments/{enrollment_id}/certificate"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "enrollment_not_completed");

    // Force to active then complete (bypassing payment for test simplicity).
    sqlx::query!("update enrollments set status = 'active' where id = $1", Uuid::parse_str(&enrollment_id).unwrap()).execute(&pool).await.unwrap();

    let (status, _) =
        send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments/{enrollment_id}/complete"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) =
        send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments/{enrollment_id}/certificate"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    let code = body["certificate_code"].as_str().unwrap().to_string();

    // Re-issuing is a conflict.
    let (status, _) =
        send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments/{enrollment_id}/certificate"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Verify is public — no token — and returns valid:true.
    let (status, body) = get(app.clone(), &format!("/certificates/{code}/verify"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["valid"], true);
    assert_eq!(body["course_title"], "Group Class");

    // Unknown code -> 200 valid:false, never 404.
    let (status, body) = get(app, "/certificates/does-not-exist/verify", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["valid"], false);
}

#[sqlx::test]
async fn assignment_submission_and_grading_flow(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor8@example.com", "tutor").await;
    let (_uid2, student_token) = insert_user_with_role(&pool, "student7@example.com", "student").await;
    let app = build_app(pool.clone());
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;
    send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;

    let (status, body) =
        send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/assignments"), Some(&tutor_token), json!({"title": "Essay 1"})).await;
    assert_eq!(status, StatusCode::CREATED);
    let assignment_id = body["id"].as_str().unwrap().to_string();

    let (status, body) =
        send(app.clone(), Method::POST, &format!("/assignments/{assignment_id}/submissions"), Some(&student_token), json!({"content": "my essay"})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["late"], false);
    let submission_id = body["id"].as_str().unwrap().to_string();

    let (status, body) =
        send(app.clone(), Method::POST, &format!("/submissions/{submission_id}/grade"), Some(&tutor_token), json!({"score": 85.5})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["score"], 85.5);

    // Resubmission after grading is rejected.
    let (status, _) = send(
        app.clone(),
        Method::POST,
        &format!("/assignments/{assignment_id}/submissions"),
        Some(&student_token),
        json!({"content": "revised"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Gradebook needs an active/completed enrollment; force it active.
    let enrollments: Vec<Uuid> = sqlx::query_scalar!("select id from enrollments where cohort_id = $1", Uuid::parse_str(&cohort_id).unwrap())
        .fetch_all(&pool)
        .await
        .unwrap();
    sqlx::query!("update enrollments set status = 'active' where id = $1", enrollments[0]).execute(&pool).await.unwrap();

    let (status, body) = get(app, &format!("/cohorts/{cohort_id}/gradebook"), Some(&tutor_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["assignment_average"], 85.5);
}

#[sqlx::test]
async fn tutor_review_requires_completed_enrollment(pool: PgPool) {
    let (tutor_uid, tutor_token) = insert_user_with_role(&pool, "tutor9@example.com", "tutor").await;
    let (_uid2, student_token) = insert_user_with_role(&pool, "student8@example.com", "student").await;
    let app = build_app(pool.clone());
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;

    let (_, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;
    let enrollment_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = send(
        app.clone(),
        Method::POST,
        &format!("/tutors/{tutor_uid}/reviews"),
        Some(&student_token),
        json!({"enrollment_id": enrollment_id, "rating": 5}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "enrollment_not_completed");

    sqlx::query!("update enrollments set status = 'completed' where id = $1", Uuid::parse_str(&enrollment_id).unwrap()).execute(&pool).await.unwrap();

    let (status, _body) = send(
        app.clone(),
        Method::POST,
        &format!("/tutors/{tutor_uid}/reviews"),
        Some(&student_token),
        json!({"enrollment_id": enrollment_id, "rating": 5, "comment": "great!"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Reputation is public.
    let (status, body) = get(app, &format!("/tutors/{tutor_uid}/reputation"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["average_rating"], 5.0);
    assert_eq!(body["review_count"], 1);
    assert!(body["avg_response_minutes"].is_null());
}
