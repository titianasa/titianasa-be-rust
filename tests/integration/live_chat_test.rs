// P39-006 (ADR-0013 L1) — Live AI Chat writes one `live_chat_question`
// event per student turn (server channel, gated behind `ai_chat_storage`
// consent). Only the student's question is stored, never the tutor's
// reply. DoD: asking 3 times in section 2 produces 3 events with the
// right `content_uid`; without consent nothing is stored, but the chat
// still works.

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

fn build_app_with_ai(pool: PgPool, ai_provider: Arc<dyn titian_backend_rust::services::ai_provider::AIProvider>) -> axum::Router {
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

/// A published Modul Belajar with 2 named sections — seeded directly
/// (not through the authoring/publish flow, which P39-002's own tests
/// already prove) since what THIS ticket needs is a real, readable
/// article for `live_chat.rs` to load, with `current_version` set so
/// `content_version` on the resulting event isn't null.
async fn seed_published_article(pool: &PgPool, label: &str) -> Uuid {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();
    let module_id: Uuid = sqlx::query_scalar!(r#"insert into modules (is_folder, subject_id, code, title) values (false, $1, $2, 'Module') returning id"#, subject_id, format!("MOD-{label}"))
        .fetch_one(pool)
        .await
        .unwrap();
    let lesson_plan = json!({
        "title": "Gerak Lurus",
        "topic": "Kinematika",
        "level": "SMA",
        "language": "id",
        "sections": [
            {"id": "sec-a", "title": "Pengantar", "goal": "Paham definisi", "content": "Gerak lurus adalah perpindahan pada lintasan lurus."},
            {"id": "sec-b", "title": "Rumus", "goal": "Bisa menghitung", "content": "v = s/t"},
        ],
    });
    sqlx::query_scalar!(
        r#"insert into module_items (module_id, node_type, title, content_type, status, lesson_plan, current_version)
           values ($1, 'item', 'Gerak Lurus', 'article', 'published', $2, 1) returning id"#,
        module_id,
        lesson_plan,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn turn_body(item_id: Uuid, lesson_plan: &Value, section_index: usize, message: &str) -> Value {
    json!({
        "item_id": item_id,
        "lesson_plan": lesson_plan,
        "section_index": section_index,
        "language": "id",
        "message": message,
        "history": [],
    })
}

async fn lesson_plan_of(pool: &PgPool, item_id: Uuid) -> Value {
    sqlx::query_scalar!(r#"select lesson_plan as "lesson_plan!" from module_items where id = $1"#, item_id).fetch_one(pool).await.unwrap()
}

async fn grant_ai_chat_storage(app: axum::Router, token: &str) {
    let (status, body) = send(app, Method::POST, "/me/consents", token, json!({"kind": "ai_chat_storage", "granted": true})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
}

#[sqlx::test]
async fn asking_a_question_with_consent_records_a_live_chat_question_event(pool: PgPool) {
    let (uid, token) = insert_user_with_role(&pool, "livechat1@example.com", "student").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success("Betul, itu jawaban yang tepat!")));
    grant_ai_chat_storage(app.clone(), &token).await;

    let item_id = seed_published_article(&pool, "LC1").await;
    let plan = lesson_plan_of(&pool, item_id).await;

    let (status, body) = send(app, Method::POST, "/ai/live-chat-turn", &token, turn_body(item_id, &plan, 0, "Apa itu gerak lurus?")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["reply"], "Betul, itu jawaban yang tepat!");

    let row = sqlx::query!(
        r#"select payload, source, module_item_id, content_uid, content_version, user_id
           from learning_events where event_type = 'live_chat_question'"#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.user_id, uid);
    assert_eq!(row.source, "live_ai_chat");
    assert_eq!(row.module_item_id, Some(item_id));
    assert_eq!(row.content_uid.as_deref(), Some("sec-a"), "section 0's id from the lesson_plan, not a raw index");
    assert_eq!(row.content_version, Some(1));
    assert_eq!(row.payload["question"], "Apa itu gerak lurus?");
    // The tutor's reply is never stored — only the student's question.
    assert!(row.payload.get("reply").is_none());
    assert!(row.payload.as_object().unwrap().len() == 1, "payload should only carry the question key: {:?}", row.payload);
}

#[sqlx::test]
async fn asking_three_times_in_section_two_records_three_events_with_that_sections_id(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "livechat2@example.com", "student").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success("Jawaban tutor.")));
    grant_ai_chat_storage(app.clone(), &token).await;

    let item_id = seed_published_article(&pool, "LC2").await;
    let plan = lesson_plan_of(&pool, item_id).await;

    for msg in ["Apa itu v?", "Kenapa s/t?", "Contohnya apa?"] {
        let (status, body) = send(app.clone(), Method::POST, "/ai/live-chat-turn", &token, turn_body(item_id, &plan, 1, msg)).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
    }

    let rows = sqlx::query!(r#"select content_uid, payload from learning_events where event_type = 'live_chat_question' order by created_at asc"#).fetch_all(&pool).await.unwrap();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.content_uid.as_deref(), Some("sec-b"), "section index 1's id");
    }
    assert_eq!(rows[0].payload["question"], "Apa itu v?");
    assert_eq!(rows[1].payload["question"], "Kenapa s/t?");
    assert_eq!(rows[2].payload["question"], "Contohnya apa?");
}

#[sqlx::test]
async fn without_consent_no_event_is_recorded_but_the_chat_still_works(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "livechat3@example.com", "student").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success("Tetap bisa menjawab meski tidak dicatat.")));
    // Deliberately no grant_ai_chat_storage call.

    let item_id = seed_published_article(&pool, "LC3").await;
    let plan = lesson_plan_of(&pool, item_id).await;

    let (status, body) = send(app, Method::POST, "/ai/live-chat-turn", &token, turn_body(item_id, &plan, 0, "Tolong jelaskan lagi.")).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["reply"], "Tetap bisa menjawab meski tidak dicatat.");

    let count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from learning_events where event_type = 'live_chat_question'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn the_streaming_turn_also_records_the_question_event(pool: PgPool) {
    let (uid, token) = insert_user_with_role(&pool, "livechat4@example.com", "student").await;
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success("Balasan streaming.")));
    grant_ai_chat_storage(app.clone(), &token).await;

    let item_id = seed_published_article(&pool, "LC4").await;
    let plan = lesson_plan_of(&pool, item_id).await;

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/ai/live-chat-turn/stream")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(turn_body(item_id, &plan, 0, "Pertanyaan lewat stream").to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("Balasan streaming."), "{text}");

    let row = sqlx::query!(r#"select user_id, content_uid, payload from learning_events where event_type = 'live_chat_question'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(row.user_id, uid);
    assert_eq!(row.content_uid.as_deref(), Some("sec-a"));
    assert_eq!(row.payload["question"], "Pertanyaan lewat stream");
}
