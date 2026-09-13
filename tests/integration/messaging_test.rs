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
        payment_provider: Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        ai_provider: Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        text_ai_provider: Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        meeting_provider: Arc::new(titian_backend_rust::services::meeting_provider::StubMeetingProvider),
        storage: Arc::new(titian_backend_rust::services::storage::InMemoryStorage::new()),
        canvas_hub: Arc::new(titian_backend_rust::services::canvas_hub::CanvasHub::new()),
        collab_hub: Arc::new(titian_backend_rust::services::collab_hub::CollabHub::new()),
    });
    routes::create_router(state)
}

async fn insert_user_with_role(pool: &PgPool, email: &str, role: &str) -> (Uuid, String, Uuid) {
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
    (user_id, token, org_id)
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

// Sets up: a tutor-owned group product + cohort, and enrolls `student_uid`.
async fn create_cohort_and_enroll(app: axum::Router, tutor_token: &str, student_token: &str) -> String {
    let (_, product) = send(app.clone(), Method::POST, "/tutors/me/products", Some(tutor_token), json!({"type": "group", "title": "Group", "price_idr": 100000, "capacity": 5})).await;
    let product_id = product["id"].as_str().unwrap().to_string();
    let (_, cohort) = send(app.clone(), Method::POST, &format!("/products/{product_id}/cohorts"), Some(tutor_token), json!({"name": "Batch 1"})).await;
    let cohort_id = cohort["id"].as_str().unwrap().to_string();
    send(app, Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(student_token), json!({})).await;
    cohort_id
}

#[sqlx::test]
async fn open_conversation_self_and_idempotent(pool: PgPool) {
    let (_tutor_uid, tutor_token, _) = insert_user_with_role(&pool, "tutor1@example.com", "tutor").await;
    let (_student_uid, student_token, _) = insert_user_with_role(&pool, "student1@example.com", "student").await;
    let app = build_app(pool);
    let cohort_id = create_cohort_and_enroll(app.clone(), &tutor_token, &student_token).await;

    let (status, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&student_token), json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    let id1 = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["cohort_id"], cohort_id);

    // Idempotent — same id on a repeat open.
    let (status, body2) = send(app, Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&student_token), json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body2["id"], id1);
}

#[sqlx::test]
async fn open_conversation_permissions(pool: PgPool) {
    let (tutor_uid, tutor_token, _) = insert_user_with_role(&pool, "tutor2@example.com", "tutor").await;
    let (student_uid, student_token, _) = insert_user_with_role(&pool, "student2@example.com", "student").await;
    let (_outsider_uid, outsider_token, _) = insert_user_with_role(&pool, "outsider2@example.com", "student").await;
    let app = build_app(pool);
    let cohort_id = create_cohort_and_enroll(app.clone(), &tutor_token, &student_token).await;

    // A non-enrolled student opening for themself -> 403.
    let (status, _) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&outsider_token), json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Tutor (manager) opens on behalf of the named, enrolled student.
    let (status, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&tutor_token), json!({"student_id": student_uid.to_string()})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    assert_eq!(body["student_id"], student_uid.to_string());

    // Tutor without student_id -> 422.
    let (status, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "student_id_required");

    // Tutor naming a never-enrolled student -> 404, not 403.
    let (status, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&tutor_token), json!({"student_id": Uuid::new_v4().to_string()})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "student_not_enrolled");
    let _ = tutor_uid;
}

#[sqlx::test]
async fn messages_read_send_permissions_and_unread_count(pool: PgPool) {
    let (_tutor_uid, tutor_token, _) = insert_user_with_role(&pool, "tutor3@example.com", "tutor").await;
    let (_student_uid, student_token, _) = insert_user_with_role(&pool, "student3@example.com", "student").await;
    let (_outsider_uid, outsider_token, _) = insert_user_with_role(&pool, "outsider3@example.com", "student").await;
    let app = build_app(pool);
    let cohort_id = create_cohort_and_enroll(app.clone(), &tutor_token, &student_token).await;
    let (_, convo) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&student_token), json!({})).await;
    let convo_id = convo["id"].as_str().unwrap().to_string();

    // Whitespace-only body -> 422.
    let (status, body) = send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/messages"), Some(&student_token), json!({"body": "   "})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "empty_message");

    // Student sends 2 messages.
    send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/messages"), Some(&student_token), json!({"body": "hello"})).await;
    let (status, msg2) = send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/messages"), Some(&student_token), json!({"body": "  are you there?  "})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(msg2["body"], "are you there?"); // trimmed

    // Outsider cannot read or send.
    let (status, _) = get(app.clone(), &format!("/conversations/{convo_id}/messages"), Some(&outsider_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/messages"), Some(&outsider_token), json!({"body": "hi"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Tutor reads the thread.
    let (status, thread) = get(app.clone(), &format!("/conversations/{convo_id}/messages"), Some(&tutor_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(thread["items"].as_array().unwrap().len(), 2);
    assert_eq!(thread["conversation"]["student_name"], "Test User");
    assert_eq!(thread["conversation"]["tutor_name"], "Test User");

    // Tutor's conversation list shows unread_count: 2.
    let (_, list) = get(app.clone(), "/conversations", Some(&tutor_token)).await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["unread_count"], 2);

    // Mark read, re-fetch -> 0.
    let (status, ok) = send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/read"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ok["ok"], true);
    let (_, list2) = get(app.clone(), "/conversations", Some(&tutor_token)).await;
    assert_eq!(list2["items"][0]["unread_count"], 0);

    // Unauthenticated -> 401.
    let (status, _) = get(app, "/conversations", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// Builds a module + one item via the real content-authoring API (a
// fresh curriculum_developer user + subject each call, code suffixed by
// `label` to avoid unique-code collisions across calls in the same
// test), with the given `content_type`. canvas_service only checks
// content_type, not status, so the item stays 'draft'.
async fn create_lesson(app: axum::Router, pool: &PgPool, label: &str, content_type: &str) -> Uuid {
    let (_uid, dev_token, _org) = insert_user_with_role(pool, &format!("dev-{label}@example.com"), "curriculum_developer").await;
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();

    let (_, module) = send(app.clone(), Method::POST, "/modules", Some(&dev_token), json!({"is_folder": false, "subject_id": subject_id, "code": format!("MOD-{label}"), "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();
    let (status, item) = send(
        app,
        Method::POST,
        &format!("/modules/{module_id}/items"),
        Some(&dev_token),
        json!({"node_type": "item", "title": "Test Item", "content_type": content_type, "format": "markdown", "content": "# Test\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{item:?}");
    Uuid::parse_str(item["id"].as_str().unwrap()).unwrap()
}

#[sqlx::test]
async fn canvas_session_create_list_get(pool: PgPool) {
    let (_tutor_uid, tutor_token, _) = insert_user_with_role(&pool, "tutor4@example.com", "tutor").await;
    let (_student_uid, student_token, _) = insert_user_with_role(&pool, "student4@example.com", "student").await;
    let (_outsider_uid, outsider_token, _) = insert_user_with_role(&pool, "outsider4@example.com", "student").await;
    let app = build_app(pool.clone());
    let cohort_id = create_cohort_and_enroll(app.clone(), &tutor_token, &student_token).await;
    let (_, convo) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&student_token), json!({})).await;
    let convo_id = convo["id"].as_str().unwrap().to_string();

    let writing_lesson_id = create_lesson(app.clone(), &pool, "canvas-w", "writing").await;
    let non_writing_lesson_id = create_lesson(app.clone(), &pool, "canvas-nw", "learn").await;

    // Non-writing lesson -> 422.
    let (status, body) = send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/canvas-sessions"), Some(&student_token), json!({"item_id": non_writing_lesson_id})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "not_a_writing_item");

    // Outsider cannot create.
    let (status, _) = send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/canvas-sessions"), Some(&outsider_token), json!({"item_id": writing_lesson_id})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Student creates a session for a writing lesson.
    let (status, session) = send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/canvas-sessions"), Some(&student_token), json!({"item_id": writing_lesson_id})).await;
    assert_eq!(status, StatusCode::CREATED, "{session:?}");
    assert_eq!(session["mode"], "learning");
    assert_eq!(session["status"], "active");
    assert_eq!(session["content"], "");
    let session_id = session["id"].as_str().unwrap().to_string();

    // Tutor discovers the active session without knowing its id upfront.
    let (status, list) = get(app.clone(), &format!("/conversations/{convo_id}/canvas-sessions"), Some(&tutor_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["status"], "active");

    // Outsider can't list.
    let (status, _) = get(app.clone(), &format!("/conversations/{convo_id}/canvas-sessions"), Some(&outsider_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // GET single session: outsider 403, participant (tutor) 200.
    let (status, _) = get(app.clone(), &format!("/canvas-sessions/{session_id}"), Some(&outsider_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, detail) = get(app, &format!("/canvas-sessions/{session_id}"), Some(&tutor_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["session"]["id"], session_id);
    assert_eq!(detail["events"].as_array().unwrap().len(), 0);
}

// Mirrors Bun's "canvas_service mode enforcement" describe block, which
// calls canvas_service.ts functions directly rather than over HTTP —
// this is the same logic canvas_ws.ts's message handler calls for
// document_updated/mode_changed, so exercising it directly is a
// faithful equivalent without needing a live WebSocket connection.
#[sqlx::test]
async fn mode_enforcement_and_event_persistence(pool: PgPool) {
    let (tutor_uid, tutor_token, _) = insert_user_with_role(&pool, "tutor5@example.com", "tutor").await;
    let (student_uid, student_token, _) = insert_user_with_role(&pool, "student5@example.com", "student").await;
    let app = build_app(pool.clone());
    let cohort_id = create_cohort_and_enroll(app.clone(), &tutor_token, &student_token).await;
    let (_, convo) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&student_token), json!({})).await;
    let convo_id = Uuid::parse_str(convo["id"].as_str().unwrap()).unwrap();
    let writing_lesson_id = create_lesson(app.clone(), &pool, "mode-enf", "writing").await;

    let ctx = titian_backend_rust::models::auth::AuthContext { user_id: student_uid, organization_id: None, role: None };
    let session = titian_backend_rust::services::canvas::create_session(&pool, &ctx, convo_id, writing_lesson_id).await.unwrap();
    let conversation = titian_backend_rust::services::conversation::find_by_id(&pool, convo_id).await.unwrap().unwrap();

    // Tutor edits in 'learning' mode -> allowed, version becomes 1.
    let updated = titian_backend_rust::services::canvas::apply_document_update(&pool, &session, &conversation, tutor_uid, "tutor draft").await.unwrap();
    assert_eq!(updated.version, 1);
    assert_eq!(updated.content, "tutor draft");

    // Tutor switches mode to 'assessment' (tutor-only action, succeeds).
    let updated = titian_backend_rust::services::canvas::apply_mode_change(&pool, &updated, &conversation, tutor_uid, "assessment").await.unwrap();
    assert_eq!(updated.mode, "assessment");

    // Only the tutor may change mode — student attempt rejected.
    let err = titian_backend_rust::services::canvas::apply_mode_change(&pool, &updated, &conversation, student_uid, "exam").await;
    assert!(err.is_err());

    // Tutor edit now rejected outside learning mode.
    let err = titian_backend_rust::services::canvas::apply_document_update(&pool, &updated, &conversation, tutor_uid, "sneaky tutor edit").await;
    assert!(err.is_err());

    // Student's own edit still succeeds regardless of mode.
    let updated = titian_backend_rust::services::canvas::apply_document_update(&pool, &updated, &conversation, student_uid, "student writes on").await.unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.content, "student writes on");

    // A comment + the mode change above wrote canvas_events rows;
    // document_updated (x2) contributed none.
    titian_backend_rust::services::canvas::apply_comment(&pool, &updated, tutor_uid, "nice work", Some(3)).await.unwrap();
    let event_types: Vec<String> = sqlx::query_scalar!(r#"select type from canvas_events where session_id = $1 order by type"#, session.id).fetch_all(&pool).await.unwrap();
    assert_eq!(event_types, vec!["comment_created".to_string(), "mode_changed".to_string()]);
}

// P22-002 — GET /tutors/{id}/reputation's avg_response_minutes/
// response_under_1h_rate, now that R10 (conversations/messages) exists
// to compute them from. A fresh tutor with zero conversations still
// gets null (not 0) — already covered by marketplace_test.rs's own
// reputation test; this covers the "real reply pattern" side.
#[sqlx::test]
async fn reputation_response_time_from_real_conversation(pool: PgPool) {
    let (tutor_uid, tutor_token, _) = insert_user_with_role(&pool, "tutor6@example.com", "tutor").await;
    let (_student_uid, student_token, _) = insert_user_with_role(&pool, "student6@example.com", "student").await;
    let app = build_app(pool);
    let cohort_id = create_cohort_and_enroll(app.clone(), &tutor_token, &student_token).await;
    let (_, convo) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), Some(&student_token), json!({})).await;
    let convo_id = convo["id"].as_str().unwrap().to_string();

    // Student asks, tutor replies promptly (well under an hour).
    send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/messages"), Some(&student_token), json!({"body": "any homework today?"})).await;
    send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/messages"), Some(&tutor_token), json!({"body": "yes, chapter 3"})).await;

    let (status, reputation) = get(app, &format!("/tutors/{tutor_uid}/reputation"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(reputation["avg_response_minutes"].as_i64().is_some());
    assert!(reputation["avg_response_minutes"].as_i64().unwrap() < 60);
    assert_eq!(reputation["response_under_1h_rate"], 1.0);
}
