// P40-003 (ADR-0013 §3.5, ADR-0014 §2 "Pengaturan AI") — the resolver's
// own DoD, proven directly against a real database:
//   1. an empty ai_role_settings table resolves to EXACTLY the same
//      model every call site read from Config before this ticket;
//   2. saving a role setting is live on the very NEXT resolve() call —
//      no restart, no stale cache;
//   3. a model missing a role's required capability (e.g. no `vision`
//      for `ocr`) is rejected with 422;
//   4. every /admin/ai/* endpoint is gated to platform_admin, and every
//      write lands in the audit log.

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
    services::{ai_provider::FakeAIProvider, ai_settings, token},
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
        ai_speaking_room_text_model: "gemini-3.8-flash".into(),
        ai_speaking_room_tts_model: "hexgrad/kokoro-82m".into(),
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
        ai_writing_evaluation_model: "gemini-3.8-flash".into(),
        ai_speaking_evaluation_model: "gemini-3.8-flash".into(),
        ai_grammar_evaluation_credit_cost: 1,
        ai_grammar_evaluation_model: "gemini-3.8-flash".into(),
        // Deliberately NOT the seeded catalog's id — a sentinel so
        // test A can prove `resolve` returns exactly THIS, not
        // something else it coincidentally matches.
        ai_lesson_generation_model: "config-fallback-lesson-model-sentinel".into(),
        ai_question_generation_model: "gemini-3.8-flash".into(),
        ai_ocr_model: "gemini-3.8-flash".into(),
        ai_tts_model: "hexgrad/kokoro-82m".into(),
        redis_url: "redis://127.0.0.1:6379".into(),
        collab_checkpoint_interval_seconds: 15,
        ai_live_chat_model: "gemini-3.8-flash".into(),
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
        ai_provider: Arc::new(FakeAIProvider::success("Model ini berfungsi.")),
        text_ai_provider: Arc::new(FakeAIProvider::success("Model ini berfungsi.")),
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

fn ctx(user_id: Uuid, role: &str) -> AuthContext {
    AuthContext { user_id, organization_id: None, role: Some(role.to_string()) }
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
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

#[sqlx::test]
async fn with_an_empty_ai_role_settings_table_resolve_returns_exactly_what_config_used_to_provide(pool: PgPool) {
    let config = test_config();
    let resolved = ai_settings::resolve(&pool, &config, "lesson_generation").await.unwrap();
    assert_eq!(resolved.model_id, "config-fallback-lesson-model-sentinel");
    assert!(resolved.fallback_model_id.is_none());
}

#[sqlx::test]
async fn quiz_generation_also_falls_back_to_the_lesson_generation_config_field_when_no_row_exists(pool: PgPool) {
    // Codifies the deliberate design note in AI_ROLES: quiz_generation
    // has never had its own Config field, so today's real behavior
    // (handlers/ai.rs's old resolve_ai_model call) is preserved exactly.
    let config = test_config();
    let resolved = ai_settings::resolve(&pool, &config, "quiz_generation").await.unwrap();
    assert_eq!(resolved.model_id, "config-fallback-lesson-model-sentinel");
}

#[sqlx::test]
async fn saving_a_role_setting_is_visible_on_the_very_next_resolve_call(pool: PgPool) {
    let config = test_config();
    let (admin_uid, _) = insert_user_with_role(&pool, "aisettings1@example.com", "platform_admin").await;
    let admin_ctx = ctx(admin_uid, "platform_admin");

    // Populate the cache from the (currently empty) table first.
    let before = ai_settings::resolve(&pool, &config, "lesson_generation").await.unwrap();
    assert_eq!(before.model_id, "config-fallback-lesson-model-sentinel");

    ai_settings::save_role(
        &pool,
        &admin_ctx,
        "lesson_generation",
        ai_settings::SaveRoleInput { model_id: Some("gemini-3.8-flash".into()), fallback_model_id: None, temperature: Some(0.5), max_tokens: Some(4096), daily_token_budget: None, enabled: true, reason: Some("uji coba".into()) },
    )
    .await
    .unwrap();

    // No restart, no explicit cache-clear call from the test itself —
    // this is the whole point of invalidate_cache() firing inside
    // save_role.
    let after = ai_settings::resolve(&pool, &config, "lesson_generation").await.unwrap();
    assert_eq!(after.model_id, "gemini-3.8-flash", "must reflect the just-saved override immediately");
    assert_eq!(after.temperature, Some(0.5));
    assert_eq!(after.max_tokens, Some(4096));
}

#[sqlx::test]
async fn a_model_missing_the_roles_required_capability_is_rejected(pool: PgPool) {
    let (admin_uid, _) = insert_user_with_role(&pool, "aisettings2@example.com", "platform_admin").await;
    let admin_ctx = ctx(admin_uid, "platform_admin");

    // openai/whisper-1 is seeded with only ["stt"] — ocr requires vision.
    let result = ai_settings::save_role(
        &pool,
        &admin_ctx,
        "ocr",
        ai_settings::SaveRoleInput { model_id: Some("openai/whisper-1".into()), fallback_model_id: None, temperature: None, max_tokens: None, daily_token_budget: None, enabled: true, reason: None },
    )
    .await;

    let err = result.expect_err("a model without vision must be rejected for the ocr role");
    match err {
        titian_backend_rust::errors::AppError::UnprocessableEntity(code, _) => assert_eq!(code, "model_missing_capability"),
        other => panic!("expected UnprocessableEntity(model_missing_capability), got {other:?}"),
    }
}

#[sqlx::test]
async fn a_model_not_in_the_catalog_at_all_is_rejected(pool: PgPool) {
    let (admin_uid, _) = insert_user_with_role(&pool, "aisettings3@example.com", "platform_admin").await;
    let admin_ctx = ctx(admin_uid, "platform_admin");

    let result = ai_settings::save_role(
        &pool,
        &admin_ctx,
        "lesson_generation",
        ai_settings::SaveRoleInput { model_id: Some("made-up-model-that-does-not-exist".into()), fallback_model_id: None, temperature: None, max_tokens: None, daily_token_budget: None, enabled: true, reason: None },
    )
    .await;
    let err = result.expect_err("an unknown model_id must be rejected");
    match err {
        titian_backend_rust::errors::AppError::UnprocessableEntity(code, _) => assert_eq!(code, "model_not_in_catalog"),
        other => panic!("expected UnprocessableEntity(model_not_in_catalog), got {other:?}"),
    }
}

#[sqlx::test]
async fn platform_admin_can_read_and_write_ai_settings_over_http_and_it_is_audited(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "aisettings4@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let (status, catalog) = get(app.clone(), "/admin/ai/catalog", &token).await;
    assert_eq!(status, StatusCode::OK, "{catalog:?}");
    let catalog_items = catalog.as_array().unwrap();
    assert!(catalog_items.iter().any(|c| c["model_id"] == "gemini-3.8-flash"), "seed row must be present: {catalog_items:?}");

    let (status, roles) = get(app.clone(), "/admin/ai/roles", &token).await;
    assert_eq!(status, StatusCode::OK, "{roles:?}");
    let role_items = roles.as_array().unwrap();
    assert_eq!(role_items.len(), titian_backend_rust::services::ai_settings::AI_ROLES.len(), "every registered role appears even with no row yet");

    let (status, saved) = send(
        app.clone(),
        Method::PUT,
        "/admin/ai/roles/lesson_generation",
        &token,
        json!({"model_id": "gemini-3.8-flash", "fallback_model_id": null, "temperature": 0.4, "max_tokens": 8000, "daily_token_budget": null, "enabled": true, "reason": "pindah ke Gemini lewat Admin Pusat"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved:?}");
    assert_eq!(saved["model_id"], "gemini-3.8-flash");

    // The write must show up in the audit log.
    let (status, audit) = get(app.clone(), "/admin/audit-log", &token).await;
    assert_eq!(status, StatusCode::OK);
    let audit_items = audit["items"].as_array().unwrap();
    assert!(audit_items.iter().any(|a| a["action"] == "ai_role_settings.updated" && a["reason"] == "pindah ke Gemini lewat Admin Pusat"), "{audit_items:?}");
}

#[sqlx::test]
async fn other_roles_get_403_on_every_ai_settings_endpoint(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "aisettings5@example.com", "curriculum_developer").await;
    let app = build_app(pool.clone());

    let (status, _) = get(app.clone(), "/admin/ai/catalog", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = get(app.clone(), "/admin/ai/roles", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(app.clone(), Method::PUT, "/admin/ai/roles/lesson_generation", &token, json!({"model_id": null, "fallback_model_id": null, "temperature": null, "max_tokens": null, "daily_token_budget": null, "enabled": true})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(app.clone(), Method::PATCH, "/admin/ai/catalog/00000000-0000-0000-0000-000000000000", &token, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn disabling_a_catalog_model_removes_it_from_get_ai_models(pool: PgPool) {
    let (_uid, admin_token) = insert_user_with_role(&pool, "aisettings6@example.com", "platform_admin").await;
    let (_uid2, dev_token) = insert_user_with_role(&pool, "aisettings6dev@example.com", "curriculum_developer").await;
    let app = build_app(pool.clone());

    let (status, before) = get(app.clone(), "/ai/models", &dev_token).await;
    assert_eq!(status, StatusCode::OK, "{before:?}");
    assert!(before["models"].as_array().unwrap().iter().any(|m| m["id"] == "gemini-3.8-flash"));

    let id: Uuid = sqlx::query_scalar!(r#"select id from ai_model_catalog where model_id = 'gemini-3.8-flash'"#).fetch_one(&pool).await.unwrap();
    let (status, patched) = send(app.clone(), Method::PATCH, &format!("/admin/ai/catalog/{id}"), &admin_token, json!({"enabled": false, "reason": "sedang diperbaiki"})).await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");
    assert_eq!(patched["enabled"], false);

    let (status, after) = get(app.clone(), "/ai/models", &dev_token).await;
    assert_eq!(status, StatusCode::OK, "{after:?}");
    assert!(!after["models"].as_array().unwrap().iter().any(|m| m["id"] == "gemini-3.8-flash"), "a disabled model must disappear from the author's picker: {after:?}");
}

#[sqlx::test]
async fn testing_a_role_returns_latency_and_sample_output(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "aisettings7@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    let (status, body) = send(app.clone(), Method::POST, "/admin/ai/roles/lesson_generation/test", &token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["sample_output"], "Model ini berfungsi.");
    assert!(body["latency_ms"].as_u64().is_some());

    // stt is not text-testable — a clear error, not a silent no-op.
    let (status, body) = send(app.clone(), Method::POST, "/admin/ai/roles/stt/test", &token, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "role_not_text_testable");
}
