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

async fn insert_user(pool: &PgPool, email: &str, name: &str) -> Uuid {
    sqlx::query_scalar!(
        r#"insert into users (google_id, email, name) values ($1, $2, $3) returning id"#,
        format!("google-{email}"),
        email,
        name,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_org(pool: &PgPool, slug: &str) -> Uuid {
    sqlx::query_scalar!(
        r#"insert into organizations (name, slug, type) values ($1, $1, 'school') returning id"#,
        slug,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_role(pool: &PgPool, user_id: Uuid, org_id: Uuid, role: &str) {
    sqlx::query!(
        r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, $3)"#,
        user_id,
        org_id,
        role,
    )
    .execute(pool)
    .await
    .unwrap();
}

fn access_token_for(user_id: Uuid, email: &str) -> String {
    token::issue_access_token(JWT_SECRET, &user_id.to_string(), email, 15).unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[sqlx::test]
async fn get_members_forbidden_for_non_admin_role(pool: PgPool) {
    let org_id = insert_org(&pool, "org-a").await;
    let user_id = insert_user(&pool, "student@example.com", "Student").await;
    insert_role(&pool, user_id, org_id, "student").await;
    let token = access_token_for(user_id, "student@example.com");

    let app = build_app(pool);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/organizations/{org_id}/members"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-organization-id", org_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn get_members_returns_list_for_org_owner(pool: PgPool) {
    let org_id = insert_org(&pool, "org-b").await;
    let owner_id = insert_user(&pool, "owner@example.com", "Owner").await;
    insert_role(&pool, owner_id, org_id, "org_owner").await;
    let member_id = insert_user(&pool, "member@example.com", "Member").await;
    insert_role(&pool, member_id, org_id, "teacher").await;
    let token = access_token_for(owner_id, "owner@example.com");

    let app = build_app(pool);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/organizations/{org_id}/members"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-organization-id", org_id.to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
}

#[sqlx::test]
async fn assign_tutor_creates_role_and_profile(pool: PgPool) {
    let org_id = insert_org(&pool, "org-c").await;
    let owner_id = insert_user(&pool, "owner2@example.com", "Owner2").await;
    insert_role(&pool, owner_id, org_id, "org_owner").await;
    let target_id = insert_user(&pool, "newtutor@example.com", "New Tutor").await;
    let token = access_token_for(owner_id, "owner2@example.com");

    let app = build_app(pool.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/organizations/{org_id}/tutors"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-organization-id", org_id.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"user_id": target_id, "bio": "Hi there", "specializations": ["math"]}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = body_json(response).await;
    assert_eq!(body["bio"], "Hi there");
    assert_eq!(body["specializations"][0], "math");

    let role: String = sqlx::query_scalar!(
        r#"select role from user_organization_roles where user_id = $1 and organization_id = $2"#,
        target_id,
        org_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(role, "tutor");
}

#[sqlx::test]
async fn assign_tutor_forbidden_for_student(pool: PgPool) {
    let org_id = insert_org(&pool, "org-d").await;
    let student_id = insert_user(&pool, "student2@example.com", "Student2").await;
    insert_role(&pool, student_id, org_id, "student").await;
    let target_id = insert_user(&pool, "target@example.com", "Target").await;
    let token = access_token_for(student_id, "student2@example.com");

    let app = build_app(pool);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/organizations/{org_id}/tutors"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-organization-id", org_id.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"user_id": target_id}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn patch_own_tutor_profile_updates_bio_leaves_specializations(pool: PgPool) {
    let org_id = insert_org(&pool, "org-e").await;
    let owner_id = insert_user(&pool, "owner3@example.com", "Owner3").await;
    insert_role(&pool, owner_id, org_id, "org_owner").await;
    let tutor_id = insert_user(&pool, "tutor@example.com", "Tutor").await;
    let owner_token = access_token_for(owner_id, "owner3@example.com");

    let app = build_app(pool.clone());
    // Seed a tutor profile via the real assign endpoint first.
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/organizations/{org_id}/tutors"))
                .header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
                .header("x-organization-id", org_id.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"user_id": tutor_id, "bio": "old bio", "specializations": ["science"]}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let tutor_token = access_token_for(tutor_id, "tutor@example.com");
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/tutors/me")
                .header(header::AUTHORIZATION, format!("Bearer {tutor_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"bio": "new bio"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["bio"], "new bio");
    // Omitted field must not be blanked out — true PATCH semantics.
    assert_eq!(body["specializations"][0], "science");
}

#[sqlx::test]
async fn patch_own_tutor_profile_404_when_not_a_tutor(pool: PgPool) {
    let user_id = insert_user(&pool, "notutor@example.com", "No Tutor").await;
    let token = access_token_for(user_id, "notutor@example.com");

    let app = build_app(pool);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/tutors/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"bio": "x"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
