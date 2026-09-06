use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use chrono::{Duration, Utc};
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
        storage: std::sync::Arc::new(titian_backend_rust::services::storage::InMemoryStorage::new()),
        canvas_hub: std::sync::Arc::new(titian_backend_rust::services::canvas_hub::CanvasHub::new()),
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

// Creates a published group cohort with 1 tutor-managed product +
// 1 cohort, ready to enroll students into.
async fn create_group_cohort(app: axum::Router, tutor_token: &str, capacity: i64) -> (String, String) {
    let (status, body) = send(app.clone(), Method::POST, "/tutors/me/products", Some(tutor_token), json!({"type": "group", "title": "Group Class", "price_idr": 100000, "capacity": capacity})).await;
    assert_eq!(status, StatusCode::CREATED, "product create failed: {body:?}");
    let product_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = send(app, Method::POST, &format!("/products/{product_id}/cohorts"), Some(tutor_token), json!({"name": "Batch 1"})).await;
    assert_eq!(status, StatusCode::CREATED, "cohort create failed: {body:?}");
    (product_id, body["id"].as_str().unwrap().to_string())
}

fn schedule_json(start: chrono::DateTime<Utc>, end: chrono::DateTime<Utc>) -> Value {
    json!({"session_date": start.format("%Y-%m-%d").to_string(), "scheduled_start": start.to_rfc3339(), "scheduled_end": end.to_rfc3339()})
}

#[sqlx::test]
async fn class_session_create_list_and_get(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor1@example.com", "tutor").await;
    let (_uid2, other_token) = insert_user_with_role(&pool, "other1@example.com", "student").await;
    let app = build_app(pool.clone());
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;

    let start = Utc::now() + Duration::hours(2);
    let end = start + Duration::hours(1);

    // An unrelated user (not the cohort's tutor, not enrolled) is 403.
    let (status, _) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/class-sessions"), Some(&other_token), schedule_json(start, end)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/class-sessions"), Some(&tutor_token), schedule_json(start, end)).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    assert_eq!(body["status"], "scheduled");
    assert_eq!(body["meeting_provider"], "stub");
    assert!(body["join_url"].as_str().unwrap().starts_with("https://stub-meet.titianasa.dev/"));
    let session_id = body["id"].as_str().unwrap().to_string();

    // scheduled_end <= scheduled_start -> 422.
    let (status, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/class-sessions"), Some(&tutor_token), schedule_json(end, start)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_schedule");

    // An enrolled (not managing) student can also list/get — schedule +
    // join_url carries no other student's private data.
    let (_, student_token) = insert_user_with_role(&pool, "student1@example.com", "student").await;
    send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;
    let (status, body) = get(app.clone(), &format!("/cohorts/{cohort_id}/class-sessions"), Some(&student_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);

    let (status, body) = get(app.clone(), &format!("/class-sessions/{session_id}"), Some(&student_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], session_id);

    // Recording status on a stub session (no real external id evidence).
    let (status, body) = get(app, &format!("/class-sessions/{session_id}/recording"), Some(&student_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "not_found");
    assert!(body["drive_url"].is_null());
}

#[sqlx::test]
async fn simulate_participant_gated_to_manager_and_stub_sessions(pool: PgPool) {
    let (tutor_uid, tutor_token) = insert_user_with_role(&pool, "tutor2@example.com", "tutor").await;
    let (_uid2, student_token) = insert_user_with_role(&pool, "student2@example.com", "student").await;
    let app = build_app(pool);
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;
    send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;

    let start = Utc::now() + Duration::hours(1);
    let end = start + Duration::hours(1);
    let (_, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/class-sessions"), Some(&tutor_token), schedule_json(start, end)).await;
    let session_id = body["id"].as_str().unwrap().to_string();

    // A plain enrolled student (non-manager) may not simulate.
    let (status, _) = send(
        app.clone(),
        Method::POST,
        &format!("/class-sessions/{session_id}/simulate-participant"),
        Some(&student_token),
        json!({"role": "student", "user_id": "00000000-0000-0000-0000-000000000000", "join_offset_minutes": 0, "duration_minutes": 60}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // role="tutor" with a user_id that isn't the cohort's actual tutor -> 422.
    let (status, body) = send(
        app.clone(),
        Method::POST,
        &format!("/class-sessions/{session_id}/simulate-participant"),
        Some(&tutor_token),
        json!({"role": "tutor", "user_id": Uuid::new_v4().to_string(), "join_offset_minutes": 0, "duration_minutes": 60}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "role_mismatch");

    // Correct tutor identity succeeds.
    let (status, _) = send(
        app,
        Method::POST,
        &format!("/class-sessions/{session_id}/simulate-participant"),
        Some(&tutor_token),
        json!({"role": "tutor", "user_id": tutor_uid.to_string(), "join_offset_minutes": 0, "duration_minutes": 60}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test]
async fn sync_attendance_computes_status_marks_absent_by_omission_and_is_idempotent(pool: PgPool) {
    let (tutor_uid, tutor_token) = insert_user_with_role(&pool, "tutor3@example.com", "tutor").await;
    let (student1_uid, student1_token) = insert_user_with_role(&pool, "student3@example.com", "student").await;
    let (_student2_uid, student2_token) = insert_user_with_role(&pool, "student4@example.com", "student").await;
    let app = build_app(pool);
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;
    send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student1_token), json!({})).await;
    send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student2_token), json!({})).await;

    // scheduled 60 minutes; student1 joins on time and stays the full
    // hour (present); student2 has NO simulated evidence at all (absent
    // by omission).
    let start = Utc::now();
    let end = start + Duration::hours(1);
    let (_, body) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/class-sessions"), Some(&tutor_token), schedule_json(start, end)).await;
    let session_id = body["id"].as_str().unwrap().to_string();

    send(
        app.clone(),
        Method::POST,
        &format!("/class-sessions/{session_id}/simulate-participant"),
        Some(&tutor_token),
        json!({"role": "tutor", "user_id": tutor_uid.to_string(), "join_offset_minutes": 0, "duration_minutes": 60}),
    )
    .await;
    send(
        app.clone(),
        Method::POST,
        &format!("/class-sessions/{session_id}/simulate-participant"),
        Some(&tutor_token),
        json!({"role": "student", "user_id": student1_uid.to_string(), "join_offset_minutes": 0, "duration_minutes": 60}),
    )
    .await;

    let (status, body) = send(app.clone(), Method::POST, &format!("/class-sessions/{session_id}/sync-attendance"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["session"]["status"], "completed");
    // 2 roster students processed: student1 (present, evidence) + student2 (absent, no evidence).
    assert_eq!(body["student_records"], 2);

    let (_, report) = get(app.clone(), &format!("/class-sessions/{session_id}/attendance"), Some(&tutor_token)).await;
    let items = report["items"].as_array().unwrap();
    assert_eq!(items.len(), 3); // tutor + 2 students
    let student1_row = items.iter().find(|r| r["user_id"] == student1_uid.to_string()).unwrap();
    assert_eq!(student1_row["verification_status"], "present");
    assert_eq!(student1_row["matched"], true);
    let student2_row = items.iter().find(|r| r["role"] == "student" && r["user_id"] != student1_uid.to_string()).unwrap();
    assert_eq!(student2_row["verification_status"], "absent");
    assert_eq!(student2_row["matched"], false);

    // A plain enrolled student sees only the tutor row + their own row.
    let (_, own_report) = get(app.clone(), &format!("/class-sessions/{session_id}/attendance"), Some(&student1_token)).await;
    assert_eq!(own_report["items"].as_array().unwrap().len(), 2);

    // cohort attendance_records reflect the derived decision, method='online'.
    let (_, attendance) = get(app.clone(), &format!("/cohorts/{cohort_id}/attendance"), Some(&tutor_token)).await;
    let records = attendance["items"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|r| r["method"] == "online" && r["marked_by"].is_null()));

    // Idempotent: re-running sync produces the same 2 student_records,
    // no duplicate attendance_records rows.
    let (status, body) = send(app.clone(), Method::POST, &format!("/class-sessions/{session_id}/sync-attendance"), Some(&tutor_token), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["student_records"], 2);
    let (_, attendance2) = get(app, &format!("/cohorts/{cohort_id}/attendance"), Some(&tutor_token)).await;
    assert_eq!(attendance2["items"].as_array().unwrap().len(), 2);
}

#[sqlx::test]
async fn manual_attendance_mark_is_idempotent_and_validates(pool: PgPool) {
    let (_uid, tutor_token) = insert_user_with_role(&pool, "tutor4@example.com", "tutor").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "student5@example.com", "student").await;
    let (_uid2, other_tutor_token) = insert_user_with_role(&pool, "tutor5@example.com", "tutor").await;
    let app = build_app(pool);
    let (_product_id, cohort_id) = create_group_cohort(app.clone(), &tutor_token, 5).await;
    send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), Some(&student_token), json!({})).await;

    let session_date = "2026-09-10";

    // An unrelated tutor (not owner, not org admin of this cohort) is 403.
    let (status, _) = send(
        app.clone(),
        Method::POST,
        &format!("/cohorts/{cohort_id}/sessions/{session_date}/attendance"),
        Some(&other_tutor_token),
        json!({"records": [{"student_id": student_uid.to_string(), "status": "present"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Invalid status -> 422; 'partial' is DB-legal but not manually settable.
    let (status, body) = send(
        app.clone(),
        Method::POST,
        &format!("/cohorts/{cohort_id}/sessions/{session_date}/attendance"),
        Some(&tutor_token),
        json!({"records": [{"student_id": student_uid.to_string(), "status": "partial"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_attendance_status");

    // Mark present.
    let (status, body) = send(
        app.clone(),
        Method::POST,
        &format!("/cohorts/{cohort_id}/sessions/{session_date}/attendance"),
        Some(&tutor_token),
        json!({"records": [{"student_id": student_uid.to_string(), "status": "present"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["items"][0]["status"], "present");
    assert_eq!(body["items"][0]["method"], "manual");

    // Re-marking the same (cohort, student, date) updates in place, not a new row.
    let (status, body) = send(
        app.clone(),
        Method::POST,
        &format!("/cohorts/{cohort_id}/sessions/{session_date}/attendance"),
        Some(&tutor_token),
        json!({"records": [{"student_id": student_uid.to_string(), "status": "late"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["status"], "late");

    let (_, all) = get(app.clone(), &format!("/cohorts/{cohort_id}/attendance"), Some(&tutor_token)).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 1);

    // Student sees only their own row.
    let (_, own) = get(app, &format!("/cohorts/{cohort_id}/attendance"), Some(&student_token)).await;
    assert_eq!(own["items"].as_array().unwrap().len(), 1);
    assert_eq!(own["items"][0]["student_id"], student_uid.to_string());
}

#[sqlx::test]
async fn speaking_room_turn_summary_tts_and_transcribe(pool: PgPool) {
    let (_uid, token) = insert_user_with_role(&pool, "speaker1@example.com", "student").await;
    let app = build_app(pool);

    // No auth at all -> 401.
    let (status, _) = send(app.clone(), Method::POST, "/speaking-room/turn", None, json!({"user_message": "hi"})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Empty user_message -> 422.
    let (status, body) = send(app.clone(), Method::POST, "/speaking-room/turn", Some(&token), json!({"user_message": "   "})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "user_message_required");

    // reply/feedback come through verbatim from the (fake) provider's
    // JSON — camelCase, not re-keyed.
    let (status, body) = send(
        app.clone(),
        Method::POST,
        "/speaking-room/turn",
        Some(&token),
        json!({"user_message": "hello there", "language": {"code": "en", "name": "English"}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}"); // FakeAIProvider::success("{}") has no reply/feedback keys
    assert_eq!(body["error"], "ai_output_validation_failed");

    // Summary: overallScore/scores missing from "{}" -> same validation failure.
    let (status, body) = send(app.clone(), Method::POST, "/speaking-room/summary", Some(&token), json!({"messages": []})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "ai_output_validation_failed");

    // TTS: empty text -> 422; non-empty returns raw audio bytes with a content-type header.
    let (status, body) = send(app.clone(), Method::POST, "/speaking-room/tts", Some(&token), json!({"text": ""})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "text_required");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/speaking-room/tts")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(json!({"text": "halo"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get(header::CONTENT_TYPE).unwrap(), "audio/mpeg");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), &[0u8, 1, 2, 3]);

    // Transcribe: base64-decodes and returns the fake transcript.
    let audio_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"fake-audio-bytes");
    let (status, body) = send(app, Method::POST, "/speaking-room/transcribe", Some(&token), json!({"audio_base64": audio_b64})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["transcript"], "fake transcript");
}
