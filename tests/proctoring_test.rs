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
        payment_provider: Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        ai_provider: Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
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

async fn insert_subject(pool: &PgPool, code: &str) -> Uuid {
    sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, code).fetch_one(pool).await.unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn send(app: axum::Router, method: Method, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let response = app.oneshot(Request::builder().method(method).uri(uri).header(header::AUTHORIZATION, format!("Bearer {token}")).header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn get(app: axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let response = app.oneshot(Request::builder().method(Method::GET).uri(uri).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

// Builds subject -> question bank -> 1 MCQ question -> assessment
// (type='unit_test'). Questions don't need to be published for
// create_attempt to bundle them — matches the Bun original's own lack
// of a published-check there.
async fn seed_assessment(app: axum::Router, pool: &PgPool, label: &str) -> Uuid {
    let (_uid, dev_token) = insert_user_with_role(pool, &format!("dev-{label}@example.com"), "curriculum_developer").await;
    let subject_id = insert_subject(pool, &format!("PR-{label}")).await;

    let (_, bank) = send(app.clone(), Method::POST, "/question-banks", &dev_token, json!({"subject_id": subject_id, "name": "Bank"})).await;
    let bank_id = bank["id"].as_str().unwrap().to_string();
    let (_, question) = send(
        app.clone(),
        Method::POST,
        &format!("/question-banks/{bank_id}/questions"),
        &dev_token,
        json!({"type": "mcq", "difficulty": 0.5, "data": {"prompt": "1+1=?", "options": ["1", "2"]}, "correct_answer": {"index": 1}}),
    )
    .await;
    let question_id = question["id"].as_str().unwrap().to_string();

    let (_, assessment) = send(app, Method::POST, "/assessments", &dev_token, json!({"type": "unit_test", "title": "Proctored Quiz", "question_ids": [question_id]})).await;
    Uuid::parse_str(assessment["id"].as_str().unwrap()).unwrap()
}

async fn start_exam_session(app: axum::Router, student_token: &str, assessment_id: Uuid) -> Value {
    let (status, body) = send(app, Method::POST, &format!("/assessments/{assessment_id}/exam-sessions"), student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body
}

async fn create_policy(app: axum::Router, staff_token: &str, retention_days: i64) -> Uuid {
    let (status, body) = send(app, Method::POST, "/proctoring-policies", staff_token, json!({"exam_type": "unit_test", "camera": "on", "retention_days": retention_days})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
}

#[sqlx::test]
async fn policy_and_consent_first_gate(pool: PgPool) {
    let (_admin_uid, admin_token) = insert_user_with_role(&pool, "admin1@example.com", "platform_admin").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "student1@example.com", "student").await;
    let app = build_app(pool.clone());

    // Non-staff cannot create a policy.
    let (status, _) = send(app.clone(), Method::POST, "/proctoring-policies", &student_token, json!({"exam_type": "unit_test"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let policy_id = create_policy(app.clone(), &admin_token, 30).await;
    let assessment_id = seed_assessment(app.clone(), &pool, "consent").await;
    let exam_session = start_exam_session(app.clone(), &student_token, assessment_id).await;
    let exam_session_id = exam_session["exam_session_id"].as_str().unwrap().to_string();

    // Without consent -> 422, and zero rows inserted.
    let (status, body) = send(app.clone(), Method::POST, &format!("/exam-sessions/{exam_session_id}/proctoring-session"), &student_token, json!({"policy_id": policy_id, "consent": false})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "consent_required");
    let count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from proctoring_sessions where exam_session_id = $1"#, Uuid::parse_str(&exam_session_id).unwrap()).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 0);

    // A different student cannot start proctoring on someone else's exam session.
    let (_outsider_uid, outsider_token) = insert_user_with_role(&pool, "outsider1@example.com", "student").await;
    let (status, _) = send(app.clone(), Method::POST, &format!("/exam-sessions/{exam_session_id}/proctoring-session"), &outsider_token, json!({"policy_id": policy_id, "consent": true})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // With consent -> 201.
    let (status, body) = send(app, Method::POST, &format!("/exam-sessions/{exam_session_id}/proctoring-session"), &student_token, json!({"policy_id": policy_id, "consent": true})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    assert!(!body["consent_given_at"].as_str().unwrap().is_empty());
    assert_eq!(body["review_status"], "pending");
    let _ = student_uid;
}

async fn setup_session(app: axum::Router, pool: &PgPool, label: &str, retention_days: i64) -> (String, String, String, Uuid, Uuid) {
    let (_admin_uid, admin_token) = insert_user_with_role(pool, &format!("admin-{label}@example.com"), "platform_admin").await;
    let (student_uid, student_token) = insert_user_with_role(pool, &format!("student-{label}@example.com"), "student").await;
    let policy_id = create_policy(app.clone(), &admin_token, retention_days).await;
    let assessment_id = seed_assessment(app.clone(), pool, label).await;
    let exam_session = start_exam_session(app.clone(), &student_token, assessment_id).await;
    let exam_session_id = Uuid::parse_str(exam_session["exam_session_id"].as_str().unwrap()).unwrap();
    let (status, body) = send(app, Method::POST, &format!("/exam-sessions/{exam_session_id}/proctoring-session"), &student_token, json!({"policy_id": policy_id, "consent": true})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    let session_id = body["id"].as_str().unwrap().to_string();
    (admin_token, student_token, session_id, student_uid, exam_session_id)
}

#[sqlx::test]
async fn event_collector_and_owner_vs_staff_view(pool: PgPool) {
    let app = build_app(pool.clone());
    let (admin_token, student_token, session_id, _student_uid, _exam_session_id) = setup_session(app.clone(), &pool, "evt", 30).await;
    let (_intruder_uid, intruder_token) = insert_user_with_role(&pool, "intruder-evt@example.com", "student").await;

    // Owner can post an event on their own session.
    let (status, body) = send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/events"), &student_token, json!({"type": "face_missing", "severity": "high"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");

    // A different student cannot post events on someone else's session.
    let (status, _) = send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/events"), &intruder_token, json!({"type": "face_missing", "severity": "low"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // evidence_id owned by a different user -> 422.
    let intruder_asset: Value = {
        let boundary = "----b";
        let mut multipart = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
        multipart.extend_from_slice(&[0x89, 0x50, 0x4e, 0x47]);
        multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let response = app
            .clone()
            .oneshot(Request::builder().method(Method::POST).uri("/assets/upload").header(header::AUTHORIZATION, format!("Bearer {intruder_token}")).header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}")).body(Body::from(multipart)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        body_json(response).await
    };
    let intruder_asset_id = intruder_asset["id"].as_str().unwrap().to_string();
    let (status, body) = send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/events"), &student_token, json!({"type": "screen_share_detected", "severity": "medium", "evidence_id": intruder_asset_id})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_evidence");

    // Owner view: no events/risk_score keys at all.
    let (status, owner_view) = get(app.clone(), &format!("/proctoring-sessions/{session_id}"), &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(owner_view["view"], "owner");
    assert!(owner_view.get("events").is_none());
    assert!(owner_view.get("risk_score").is_none());

    // Staff view: full packet, 1 event, risk_score 7 (one high-severity event).
    let (status, staff_view) = get(app, &format!("/proctoring-sessions/{session_id}"), &admin_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(staff_view["view"], "staff");
    assert_eq!(staff_view["events"].as_array().unwrap().len(), 1);
    assert_eq!(staff_view["risk_score"], 7);
}

#[sqlx::test]
async fn risk_engine_never_auto_decides(pool: PgPool) {
    let app = build_app(pool.clone());
    let (admin_token, student_token, session_id, _student_uid, _exam_session_id) = setup_session(app.clone(), &pool, "risk", 30).await;

    // No events -> risk_score 0.
    let (_, view) = get(app.clone(), &format!("/proctoring-sessions/{session_id}"), &admin_token).await;
    assert_eq!(view["risk_score"], 0);

    // 5x severity:high -> risk_score 35, review_status still pending.
    for _ in 0..5 {
        send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/events"), &student_token, json!({"type": "multiple_faces", "severity": "high"})).await;
    }
    let (_, view) = get(app, &format!("/proctoring-sessions/{session_id}"), &admin_token).await;
    assert_eq!(view["risk_score"], 35);
    assert_eq!(view["review_status"], "pending");
}

#[sqlx::test]
async fn human_review_owner_forbidden_and_re_review_allowed(pool: PgPool) {
    let app = build_app(pool.clone());
    let (admin_token, student_token, session_id, _student_uid, _exam_session_id) = setup_session(app.clone(), &pool, "review", 30).await;

    // Even the session's own student cannot review it.
    let (status, _) = send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/review"), &student_token, json!({"decision": "cleared"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // decision:"pending" is not a valid input.
    let (status, body) = send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/review"), &admin_token, json!({"decision": "pending"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_decision");

    // Staff review.
    let (status, body) = send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/review"), &admin_token, json!({"decision": "flagged", "notes": "possible phone usage"})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["review_status"], "flagged");

    // A second, different staff re-reviews — overwrites, no conflict.
    let (_director_uid, director_token) = insert_user_with_role(&pool, "director-review@example.com", "academic_director").await;
    let (status, body) = send(app, Method::POST, &format!("/proctoring-sessions/{session_id}/review"), &director_token, json!({"decision": "cleared", "notes": "re-reviewed, false positive"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["review_status"], "cleared");
}

#[sqlx::test]
async fn retention_purges_evidence_but_keeps_the_underlying_asset(pool: PgPool) {
    let app = build_app(pool.clone());
    let (admin_token, student_token, session_id, _student_uid, _exam_session_id) = setup_session(app.clone(), &pool, "retain", 2).await;

    // Upload 2 assets (old + fresh evidence).
    let upload = |app: axum::Router, token: String| async move {
        let boundary = "----r";
        let mut multipart = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
        multipart.extend_from_slice(&[0x89, 0x50, 0x4e, 0x47]);
        multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let response = app.oneshot(Request::builder().method(Method::POST).uri("/assets/upload").header(header::AUTHORIZATION, format!("Bearer {token}")).header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}")).body(Body::from(multipart)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        body_json(response).await
    };
    let old_asset = upload(app.clone(), student_token.clone()).await;
    let old_asset_id = old_asset["id"].as_str().unwrap().to_string();
    let fresh_asset = upload(app.clone(), student_token.clone()).await;
    let fresh_asset_id = fresh_asset["id"].as_str().unwrap().to_string();

    let (_, old_event) = send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/events"), &student_token, json!({"type": "face_missing", "severity": "low", "evidence_id": old_asset_id})).await;
    let old_event_id = Uuid::parse_str(old_event["id"].as_str().unwrap()).unwrap();
    let (_, fresh_event) = send(app.clone(), Method::POST, &format!("/proctoring-sessions/{session_id}/events"), &student_token, json!({"type": "face_missing", "severity": "low", "evidence_id": fresh_asset_id})).await;
    let fresh_event_id = Uuid::parse_str(fresh_event["id"].as_str().unwrap()).unwrap();

    // Move the old event's timestamp back 3 days (retention_days=2).
    sqlx::query!(r#"update proctoring_events set "timestamp" = now() - interval '3 days' where id = $1"#, old_event_id).execute(&pool).await.unwrap();

    // GET as staff triggers the lazy purge.
    let (status, view) = get(app.clone(), &format!("/proctoring-sessions/{session_id}"), &admin_token).await;
    assert_eq!(status, StatusCode::OK);
    let events = view["events"].as_array().unwrap();
    let old = events.iter().find(|e| e["id"] == old_event_id.to_string()).unwrap();
    assert!(old["evidence_id"].is_null());
    let fresh = events.iter().find(|e| e["id"] == fresh_event_id.to_string()).unwrap();
    assert_eq!(fresh["evidence_id"], fresh_asset_id);

    let purged_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar!(r#"select purged_at from proctoring_events where id = $1"#, old_event_id).fetch_one(&pool).await.unwrap();
    assert!(purged_at.is_some());

    // The underlying asset row is never deleted.
    let (status, _) = get(app, &format!("/assets/{old_asset_id}"), &student_token).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn review_queue_listing(pool: PgPool) {
    let app = build_app(pool.clone());
    let (admin_token, student_token, _session_id, _student_uid, _exam_session_id) = setup_session(app.clone(), &pool, "queue", 30).await;

    let (status, listing) = get(app.clone(), "/proctoring-sessions", &admin_token).await;
    assert_eq!(status, StatusCode::OK, "{listing:?}");
    let items = listing["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["assessment_title"], "Proctored Quiz");
    assert_eq!(items[0]["review_status"], "pending");

    // A student cannot list the review queue.
    let (status, _) = get(app, "/proctoring-sessions", &student_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
