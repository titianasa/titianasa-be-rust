// P39-005 (ADR-0013 L1) — POST /events, the client telemetry batch
// endpoint. DoD: reading a 5-section Modul Belajar produces a
// per-section event with sane active time; without consent, nothing is
// stored.

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

fn section_read_event(module_item_id: Uuid, section_id: &str, active_seconds: f64) -> Value {
    json!({
        "event_type": "section_read",
        "source": "self_learning",
        "module_item_id": module_item_id,
        "content_uid": section_id,
        "occurred_at": chrono::Utc::now().to_rfc3339(),
        "client_event_id": Uuid::new_v4().to_string(),
        "payload": {"active_seconds": active_seconds},
    })
}

/// `learning_events.module_item_id` is a real foreign key — a client
/// event claiming to be about a module item that doesn't exist has to
/// be rejected the same as any other referential-integrity violation,
/// so these tests seed a real (if otherwise empty) one rather than
/// pointing at a made-up id.
async fn seed_module_item(pool: &PgPool, label: &str) -> Uuid {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();
    let module_id: Uuid = sqlx::query_scalar!(r#"insert into modules (is_folder, subject_id, code, title) values (false, $1, $2, 'Module') returning id"#, subject_id, format!("MOD-{label}"))
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query_scalar!(r#"insert into module_items (module_id, node_type, title, content_type) values ($1, 'item', 'Item', 'article') returning id"#, module_id).fetch_one(pool).await.unwrap()
}

async fn grant_learning_analytics(app: axum::Router, token: &str) {
    let (status, body) = send(app, Method::POST, "/me/consents", token, json!({"kind": "learning_analytics", "granted": true})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
}

#[sqlx::test]
async fn reading_five_sections_with_consent_records_five_events_with_sane_active_time(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "clientev1@example.com", "student").await;
    let app = build_app(pool.clone());
    grant_learning_analytics(app.clone(), &token).await;

    let module_item_id = seed_module_item(&pool, "CE1").await;
    let events: Vec<Value> = (1..=5).map(|n| section_read_event(module_item_id, &format!("sec{n}"), 42.0)).collect();
    let (status, body) = send(app, Method::POST, "/events", &token, json!({"events": events})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|r| r["status"] == json!("recorded")), "{results:?}");

    let count: i64 = sqlx::query_scalar!(
        r#"select count(*) as "count!" from learning_events where event_type = 'section_read' and module_item_id = $1 and (payload->>'active_seconds')::float8 = 42.0"#,
        module_item_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 5, "one event per section, each with the active-reading time it actually reported");
}

#[sqlx::test]
async fn without_consent_no_client_events_are_stored_but_the_request_still_succeeds(pool: PgPool) {
    let (uid, token) = insert_user_with_role(&pool, "clientev2@example.com", "student").await;
    let app = build_app(pool.clone());
    // Deliberately no grant_learning_analytics call — consent left at
    // its default (not granted).

    let module_item_id = Uuid::new_v4();
    let (status, body) = send(app, Method::POST, "/events", &token, json!({"events": [section_read_event(module_item_id, "sec1", 10.0)]})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["results"][0]["status"], json!("skipped_no_consent"));

    let count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from learning_events where user_id = $1 and event_type = 'section_read'"#, uid).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0, "the underlying feature (reading) works, but nothing was recorded");
}

#[sqlx::test]
async fn a_batch_over_the_size_limit_is_rejected_outright(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "clientev3@example.com", "student").await;
    let app = build_app(pool);
    let module_item_id = Uuid::new_v4();
    let events: Vec<Value> = (0..51).map(|n| section_read_event(module_item_id, &format!("sec{n}"), 1.0)).collect();

    let (status, body) = send(app, Method::POST, "/events", &token, json!({"events": events})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], json!("batch_too_large"));
}

#[sqlx::test]
async fn an_occurred_at_too_far_in_the_past_or_future_is_rejected_per_event(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "clientev4@example.com", "student").await;
    let app = build_app(pool.clone());
    grant_learning_analytics(app.clone(), &token).await;

    let module_item_id = seed_module_item(&pool, "CE4").await;
    let too_old = json!({
        "event_type": "section_read", "source": "self_learning", "module_item_id": module_item_id, "content_uid": "sec1",
        "occurred_at": (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339(),
        "client_event_id": Uuid::new_v4().to_string(), "payload": {"active_seconds": 1.0},
    });
    let too_future = json!({
        "event_type": "section_read", "source": "self_learning", "module_item_id": module_item_id, "content_uid": "sec2",
        "occurred_at": (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
        "client_event_id": Uuid::new_v4().to_string(), "payload": {"active_seconds": 1.0},
    });
    let fine = section_read_event(module_item_id, "sec3", 1.0);

    let (status, body) = send(app, Method::POST, "/events", &token, json!({"events": [too_old, too_future, fine]})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results[0]["status"], json!("rejected"));
    assert_eq!(results[1]["status"], json!("rejected"));
    assert_eq!(results[2]["status"], json!("recorded"), "a valid event in the same batch is still processed");
}

#[sqlx::test]
async fn a_server_only_event_type_cannot_be_forged_through_the_client_endpoint(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "clientev5@example.com", "student").await;
    let app = build_app(pool.clone());
    grant_learning_analytics(app.clone(), &token).await;

    let forged = json!({
        "event_type": "quiz_attempt_submitted", "source": "practice", "module_item_id": Uuid::new_v4(), "content_uid": "x",
        "occurred_at": chrono::Utc::now().to_rfc3339(), "client_event_id": Uuid::new_v4().to_string(), "payload": {},
    });
    let (status, body) = send(app, Method::POST, "/events", &token, json!({"events": [forged]})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["results"][0]["status"], json!("rejected"));
    assert!(body["results"][0]["error"].as_str().unwrap().contains("client channel"), "{body:?}");
}

#[sqlx::test]
async fn resubmitting_the_same_client_event_id_does_not_double_record(pool: PgPool) {
    let (uid, token) = insert_user_with_role(&pool, "clientev6@example.com", "student").await;
    let app = build_app(pool.clone());
    grant_learning_analytics(app.clone(), &token).await;

    let module_item_id = seed_module_item(&pool, "CE6").await;
    let client_event_id = Uuid::new_v4().to_string();
    let event = json!({
        "event_type": "hint_opened", "source": "practice", "module_item_id": module_item_id,
        "content_uid": Uuid::new_v4().to_string(), "occurred_at": chrono::Utc::now().to_rfc3339(),
        "client_event_id": client_event_id, "payload": {},
    });

    for _ in 0..2 {
        let (status, body) = send(app.clone(), Method::POST, "/events", &token, json!({"events": [event.clone()]})).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["results"][0]["status"], json!("recorded"));
    }

    let count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from learning_events where user_id = $1 and event_type = 'hint_opened'"#, uid).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1, "a retried batch (same client_event_id) must not create a second row");
}
