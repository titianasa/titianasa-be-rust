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
        ai_live_chat_model: "~deepseek/deepseek-v4-flash-latest".into(),
        gcp_project_id: "test-gcp-project".into(),
        gcp_region: "us-central1".into(),
        consent_guardian_confirmation_required: false,
    }
}

fn build_app(pool: PgPool) -> axum::Router {
    build_app_with_ai(pool, Arc::new(FakeAIProvider::success("{}")))
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

async fn insert_subject(pool: &PgPool, code: &str) -> Uuid {
    sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, code).fetch_one(pool).await.unwrap()
}

async fn insert_concept(pool: &PgPool, subject_id: Uuid, code: &str) -> Uuid {
    sqlx::query_scalar!(r#"insert into concepts (subject_id, code, name, type) values ($1, $2, $2, 'vocabulary') returning id"#, subject_id, code)
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
        .oneshot(Request::builder().method(Method::GET).uri(uri).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

// Full author -> submit-review -> publish flow for a question, so
// students can actually see/check it. Returns the published question id.
async fn create_published_question(app: axum::Router, dev_token: &str, reviewer_token: &str, bank_id: &str, concept_ids: &[Uuid], skill_category: Option<&str>, correct_index: i64) -> String {
    let mut body = json!({
        "type": "mcq",
        "difficulty": 0.5,
        "data": {"prompt": "I ___ a student.", "options": ["am", "is", "are"]},
        "correct_answer": {"index": correct_index},
        "concept_ids": concept_ids,
    });
    if let Some(sc) = skill_category {
        body["skill_category"] = json!(sc);
    }
    let (status, q) = send(app.clone(), Method::POST, &format!("/question-banks/{bank_id}/questions"), dev_token, body).await;
    assert_eq!(status, StatusCode::CREATED, "{q:?}");
    let question_id = q["id"].as_str().unwrap().to_string();

    let (status, _) = send(app.clone(), Method::POST, &format!("/questions/{question_id}/submit-review"), dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, published) = send(app, Method::POST, &format!("/questions/{question_id}/publish"), reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{published:?}");
    question_id
}

async fn create_bank(app: axum::Router, dev_token: &str, pool: &PgPool, code: &str) -> String {
    let subject_id = insert_subject(pool, code).await;
    let (_, bank) = send(app, Method::POST, "/question-banks", dev_token, json!({"subject_id": subject_id, "name": "Bank"})).await;
    bank["id"].as_str().unwrap().to_string()
}

// Full author -> submit-review -> publish flow for a module item.
// `content` deliberately has no linked grammar concept, so QA never
// blocks it.
async fn create_published_lesson(app: axum::Router, dev_token: &str, reviewer_token: &str, pool: &PgPool, label: &str, content_type: &str, concept_ids: &[Uuid]) -> String {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": format!("MOD-{label}"), "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();
    let (status, item) = send(
        app.clone(),
        Method::POST,
        &format!("/modules/{module_id}/items"),
        dev_token,
        json!({"node_type": "item", "title": format!("Item {label}"), "content_type": content_type, "format": "markdown", "content": "# Test\n", "concept_ids": concept_ids}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{item:?}");
    let item_id = item["id"].as_str().unwrap().to_string();

    let (status, _) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/submit-review"), dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, published) = send(app, Method::POST, &format!("/module-items/{item_id}/publish"), reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{published:?}");
    item_id
}

#[sqlx::test]
async fn submit_attempt_scores_grades_hooks_and_handles_missing_answers_and_double_submit(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "sub-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "sub-rev@example.com", "reviewer").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "sub-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let bank_id = create_bank(app.clone(), &dev_token, &pool, "SUB1").await;
    let concept_id = insert_concept(&pool, insert_subject(&pool, "SUB1C").await, "c1").await;
    let q1 = create_published_question(app.clone(), &dev_token, &reviewer_token, &bank_id, &[concept_id], None, 0).await;
    let q2 = create_published_question(app.clone(), &dev_token, &reviewer_token, &bank_id, &[concept_id], None, 0).await;

    let (status, assessment) = send(app.clone(), Method::POST, "/assessments", &dev_token, json!({"type": "unit_test", "title": "Unit 1 Test", "question_ids": [q1, q2]})).await;
    assert_eq!(status, StatusCode::CREATED, "{assessment:?}");
    let assessment_id = assessment["id"].as_str().unwrap().to_string();

    let (status, attempt) = send(app.clone(), Method::POST, &format!("/assessments/{assessment_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt:?}");
    let attempt_id = attempt["attempt_id"].as_str().unwrap().to_string();

    // Missing 1 of 2 required answers -> 422 with the missing question id.
    let (status, body) = send(app.clone(), Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"answers": {q1.clone(): {"index": 0}}})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "missing_required_answers");
    assert_eq!(body["missing"].as_array().unwrap(), &[Value::String(q2.clone())]);

    // Both correct -> full score, 2 learning events, submitted.
    let (status, body) = send(app.clone(), Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"answers": {q1.clone(): {"index": 0}, q2.clone(): {"index": 0}}})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["status"], "submitted");
    assert_eq!(body["score"], 100.0);
    assert_eq!(body["learning_events_created"], 2);

    // P8-001/002: XP + streak hooks fired from the handler.
    let (status, xp) = get(app.clone(), "/me/xp", &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(xp["total"], 20); // ASSESSMENT_XP.unit_test
    let (status, streak) = get(app.clone(), "/me/streak", &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(streak["current_streak"], 1);

    // Mastery/FRSS side effects for the touched concept — recomputed
    // from both learning events, both correct. Read the row directly:
    // 2 usable events gives confidence 2/5=0.4, below the 0.6 threshold
    // the GET /mastery/{id} endpoint itself gates on, so this checks the
    // underlying recompute rather than the confidence-hidden response.
    let mastery_score: f64 = sqlx::query_scalar!(r#"select score from masteries where user_id = $1 and concept_id = $2"#, student_uid, concept_id).fetch_one(&pool).await.unwrap();
    assert_eq!(mastery_score, 100.0);

    // Re-submitting an already-submitted attempt -> 409.
    let (status, body) = send(app, Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"answers": {q1: {"index": 0}, q2: {"index": 0}}})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(body["error"], "attempt_already_submitted");
}

#[sqlx::test]
async fn check_answer_awards_xp_and_triggers_rescue_after_consecutive_failures(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "chk-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "chk-rev@example.com", "reviewer").await;
    let (_student_uid, student_token) = insert_user_with_role(&pool, "chk-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let bank_id = create_bank(app.clone(), &dev_token, &pool, "CHK1").await;
    let concept_id = insert_concept(&pool, insert_subject(&pool, "CHK1C").await, "c1").await;
    let question_id = create_published_question(app.clone(), &dev_token, &reviewer_token, &bank_id, &[concept_id], Some("grammar"), 0).await;

    // A "learn" lesson linked to the same concept -> becomes the rescue
    // suggestion once the streak of failures triggers.
    let item_id = create_published_lesson(app.clone(), &dev_token, &reviewer_token, &pool, "rescue", "learn", &[concept_id]).await;

    // 2 wrong answers -> not yet triggered (threshold = 3).
    for _ in 0..2 {
        let (status, body) = send(app.clone(), Method::POST, &format!("/questions/{question_id}/check"), &student_token, json!({"submitted_answer": {"index": 1}})).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["correct"], false);
        assert_eq!(body["rescue_triggered"], false);
    }

    // 3rd consecutive wrong answer -> triggers, suggests the linked lesson.
    let (status, body) = send(app.clone(), Method::POST, &format!("/questions/{question_id}/check"), &student_token, json!({"submitted_answer": {"index": 1}})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["rescue_triggered"], true);
    assert_eq!(body["rescue_suggested_item_ids"].as_array().unwrap(), &[Value::String(item_id)]);

    // P8-001: XP for "grammar" skill_category awarded on every check
    // (no dedup reference — repeat practice is expected to earn XP each time).
    let (status, xp) = get(app.clone(), "/me/xp", &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(xp["total"], 30); // 3 x skill_xp("grammar")=10
}

#[sqlx::test]
async fn lesson_attempt_writing_submit_evaluates_and_awards_xp(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "wr-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "wr-rev@example.com", "reviewer").await;
    let (_student_uid, student_token) = insert_user_with_role(&pool, "wr-student@example.com", "student").await;

    let fake_eval = json!({
        "scores": {"task_achievement": 78, "coherence_cohesion": 82, "lexical_resource": 70, "grammar_accuracy": 75},
        "feedback": [{"quote": "he go to school", "comment": "Subject-verb agreement: use \"goes\"."}],
    });
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(fake_eval.to_string())));

    let item_id = create_published_lesson(app.clone(), &dev_token, &reviewer_token, &pool, "writing1", "writing", &[]).await;

    let (status, attempt) = send(app.clone(), Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt:?}");
    let attempt_id = attempt["attempt_id"].as_str().unwrap().to_string();

    let (status, body) = send(app.clone(), Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"answer_text": "Every day he go to school by bus."})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["status"], "evaluated");
    assert_eq!(body["evaluation"]["scores"]["task_achievement"], 78.0);
    assert_eq!(body["evaluation"]["feedback"].as_array().unwrap().len(), 1);

    let (status, xp) = get(app.clone(), "/me/xp", &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(xp["total"], 20); // skill_xp("writing")
}

#[sqlx::test]
async fn lesson_attempt_speaking_submit_requires_audio_asset_then_evaluates(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "sp-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "sp-rev@example.com", "reviewer").await;
    let (_student_uid, student_token) = insert_user_with_role(&pool, "sp-student@example.com", "student").await;

    let fake_eval = json!({
        "scores": {"grammar": 65, "vocabulary": 72, "fluency": 58, "naturalness": 60, "pronunciation": 55},
        "feedback": [{"quote": "I want eat fried rice", "comment": "Better: \"I'd like to have fried rice.\""}],
    });
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(fake_eval.to_string()).with_transcription("I want eat fried rice for lunch.")));

    let item_id = create_published_lesson(app.clone(), &dev_token, &reviewer_token, &pool, "speaking1", "speaking", &[]).await;

    let (status, attempt) = send(app.clone(), Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt:?}");
    let attempt_id = attempt["attempt_id"].as_str().unwrap().to_string();

    // Missing answer_audio_asset_id -> 422.
    let (status, body) = send(app.clone(), Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "missing_answer_audio_asset_id");

    // Upload a (fake) audio asset the student owns.
    let boundary = "----titianTestBoundary";
    let mut multipart = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"answer.webm\"\r\nContent-Type: audio/webm\r\n\r\n").into_bytes();
    multipart.extend_from_slice(b"fake-audio-bytes");
    multipart.extend_from_slice(b"\r\n");
    multipart.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/assets/upload")
                .header(header::AUTHORIZATION, format!("Bearer {student_token}"))
                .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
                .body(Body::from(multipart))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let uploaded = body_json(response).await;
    let audio_asset_id = uploaded["id"].as_str().unwrap().to_string();

    let (status, body) = send(app.clone(), Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"answer_audio_asset_id": audio_asset_id})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["status"], "evaluated");
    assert_eq!(body["transcript"], "I want eat fried rice for lunch.");
    assert_eq!(body["evaluation"]["scores"]["grammar"], 65.0);

    let (status, xp) = get(app, "/me/xp", &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(xp["total"], 15); // skill_xp("speaking")
}

#[sqlx::test]
async fn module_completion_gates_on_accuracy_and_mastery_then_skip_charges_credit(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "mc-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "mc-rev@example.com", "reviewer").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "mc-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let bank_id = create_bank(app.clone(), &dev_token, &pool, "MC1").await;
    let subject_id = insert_subject(&pool, "MC1C").await;
    let concept_id = insert_concept(&pool, subject_id, "c1").await;
    let question_id = create_published_question(app.clone(), &dev_token, &reviewer_token, &bank_id, &[concept_id], None, 0).await;
    let question_uuid = Uuid::parse_str(&question_id).unwrap();

    let item_id = create_published_lesson(app.clone(), &dev_token, &reviewer_token, &pool, "mc1", "learn", &[concept_id]).await;
    let item_uuid = Uuid::parse_str(&item_id).unwrap();
    sqlx::query!(
        r#"insert into content_blocks (item_id, type, order_index, data) values ($1, 'question_embed', 0, $2)"#,
        item_uuid,
        json!({"question_id": question_id}),
    )
    .execute(&pool)
    .await
    .unwrap();

    // Nothing answered yet -> eligible but not complete.
    let (status, status_body) = get(app.clone(), &format!("/module-items/{item_id}/completion-status"), &student_token).await;
    assert_eq!(status, StatusCode::OK, "{status_body:?}");
    assert_eq!(status_body["eligible_for_gate"], true);
    assert_eq!(status_body["accuracy"], 0.0);
    assert_eq!(status_body["required_activities_completed"], false);
    assert_eq!(status_body["completed"], false);
    assert_eq!(status_body["skip_credit_cost"], 15);

    // Answer correctly (via /check) and let mastery clear the threshold.
    let (status, _) = send(app.clone(), Method::POST, &format!("/questions/{question_id}/check"), &student_token, json!({"submitted_answer": {"index": 0}})).await;
    assert_eq!(status, StatusCode::OK);

    let (status, status_body) = get(app.clone(), &format!("/module-items/{item_id}/completion-status"), &student_token).await;
    assert_eq!(status, StatusCode::OK, "{status_body:?}");
    assert_eq!(status_body["accuracy"], 100.0);
    assert_eq!(status_body["required_activities_completed"], true);
    assert_eq!(status_body["minimum_mastery_reached"], true);
    assert_eq!(status_body["completed"], true);
    let _ = question_uuid;

    // A 2nd student, stuck below the gate, pays to skip it.
    let (student2_uid, student2_token) = insert_user_with_role(&pool, "mc-student2@example.com", "student").await;
    sqlx::query!(r#"insert into credits (user_id, balance) values ($1, 100)"#, student2_uid).execute(&pool).await.unwrap();

    let (status, skip) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/skip-completion"), &student2_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{skip:?}");
    assert_eq!(skip["credit_charged"], 15);
    let (status, balance) = get(app.clone(), "/me/credits", &student2_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance["balance"], 85);

    // Idempotent: skipping again charges nothing more.
    let (status, skip2) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/skip-completion"), &student2_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(skip2["credit_charged"], 0);

    let (status, status_body) = get(app.clone(), &format!("/module-items/{item_id}/completion-status"), &student2_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status_body["skipped"], true);
    assert_eq!(status_body["completed"], true);

    // A 3rd student without enough credit is rejected.
    let (student3_uid, student3_token) = insert_user_with_role(&pool, "mc-student3@example.com", "student").await;
    sqlx::query!(r#"insert into credits (user_id, balance) values ($1, 5)"#, student3_uid).execute(&pool).await.unwrap();
    let (status, body) = send(app, Method::POST, &format!("/module-items/{item_id}/skip-completion"), &student3_token, json!({})).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body:?}");
    assert_eq!(body["error"], "insufficient_credit");
    assert_eq!(body["required"], 15);
    assert_eq!(body["balance"], 5);
    let _ = student_uid;
}

async fn create_cohort_and_enroll(app: axum::Router, tutor_token: &str, student_token: &str) -> String {
    let (_, product) = send(app.clone(), Method::POST, "/tutors/me/products", tutor_token, json!({"type": "group", "title": "Group", "price_idr": 100000, "capacity": 5})).await;
    let product_id = product["id"].as_str().unwrap().to_string();
    let (_, cohort) = send(app.clone(), Method::POST, &format!("/products/{product_id}/cohorts"), tutor_token, json!({"name": "Batch 1"})).await;
    let cohort_id = cohort["id"].as_str().unwrap().to_string();
    send(app, Method::POST, &format!("/cohorts/{cohort_id}/enrollments"), student_token, json!({})).await;
    cohort_id
}

#[sqlx::test]
async fn canvas_submit_closes_session_and_creates_writing_evaluation(pool: PgPool) {
    let (_tutor_uid, tutor_token) = insert_user_with_role(&pool, "cv-tutor@example.com", "tutor").await;
    let (_student_uid, student_token) = insert_user_with_role(&pool, "cv-student@example.com", "student").await;
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "cv-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "cv-rev@example.com", "reviewer").await;

    let fake_eval = json!({
        "scores": {"task_achievement": 80, "coherence_cohesion": 80, "lexical_resource": 80, "grammar_accuracy": 80},
        "feedback": [{"quote": "canvas essay text", "comment": "Solid overall structure."}],
    });
    let app = build_app_with_ai(pool.clone(), Arc::new(FakeAIProvider::success(fake_eval.to_string())));

    let cohort_id = create_cohort_and_enroll(app.clone(), &tutor_token, &student_token).await;
    let (_, convo) = send(app.clone(), Method::POST, &format!("/cohorts/{cohort_id}/conversations"), &student_token, json!({})).await;
    let convo_id = convo["id"].as_str().unwrap().to_string();

    let item_id = create_published_lesson(app.clone(), &dev_token, &reviewer_token, &pool, "canvas-w", "writing", &[]).await;

    let (status, session) = send(app.clone(), Method::POST, &format!("/conversations/{convo_id}/canvas-sessions"), &student_token, json!({"item_id": item_id})).await;
    assert_eq!(status, StatusCode::CREATED, "{session:?}");
    let session_id = session["id"].as_str().unwrap().to_string();
    let session_uuid = Uuid::parse_str(&session_id).unwrap();

    // The document's content normally arrives over the WS; set it
    // directly here since this is an HTTP-only test.
    sqlx::query!(r#"update canvas_sessions set content = $2 where id = $1"#, session_uuid, "This is the canvas essay text.").execute(&pool).await.unwrap();

    let (status, body) = send(app.clone(), Method::POST, &format!("/canvas-sessions/{session_id}/submit"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["status"], "evaluated");
    assert_eq!(body["evaluation"]["scores"]["task_achievement"], 80.0);

    let (status, detail) = get(app.clone(), &format!("/canvas-sessions/{session_id}"), &student_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["session"]["status"], "closed");
    assert!(detail["session"]["submitted_attempt_id"].is_string());

    // Double-submit -> 409.
    let (status, body) = send(app, Method::POST, &format!("/canvas-sessions/{session_id}/submit"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(body["error"], "canvas_session_already_closed");
}
