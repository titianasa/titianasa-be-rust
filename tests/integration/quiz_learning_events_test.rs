// P39-004 (ADR-0013 L1) — the quiz_config path's own event writes.
// DoD: one 5-question attempt produces 1 `quiz_attempt_submitted` + 5
// `question_answered`, each with the right `content_uid`/`content_version`;
// a proctored (tryout) submission is `source = 'tryout'`.

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
    // A 204 (e.g. POST /attempts/{id}/grade) has no body at all — unlike
    // every other endpoint this file calls, which always returns JSON.
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

fn five_question_quiz() -> Value {
    let questions: Vec<Value> = (1..=5)
        .map(|n| json!({"number": n, "stem": format!("Soal {n}"), "choices": [{"label": "A", "text": "salah"}, {"label": "B", "text": "benar"}], "answer": "B"}))
        .collect();
    json!({"question_groups": [{"group_id": "g1", "type": "multiple_choice", "questions": questions}]})
}

/// Publishes a 5-question quiz item and returns (item_id, module_id).
async fn create_published_quiz(app: axum::Router, dev_token: &str, reviewer_token: &str, pool: &PgPool, label: &str) -> Uuid {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": format!("MOD-{label}"), "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();
    let (status, item) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), dev_token, json!({"node_type": "item", "title": format!("Quiz {label}"), "content_type": "quiz"})).await;
    assert_eq!(status, StatusCode::CREATED, "{item:?}");
    let item_id = item["id"].as_str().unwrap().to_string();

    let (status, patched) = send(app.clone(), Method::PATCH, &format!("/module-items/{item_id}/quiz-config"), dev_token, json!({"quiz_config": five_question_quiz()})).await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");

    let (status, _) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/submit-review"), dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, published) = send(app, Method::POST, &format!("/module-items/{item_id}/publish"), reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{published:?}");

    Uuid::parse_str(&item_id).unwrap()
}

#[sqlx::test]
async fn a_five_question_attempt_writes_one_submitted_event_and_five_question_answered_events(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "qle1-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "qle1-rev@example.com", "reviewer").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "qle1-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let item_id = create_published_quiz(app.clone(), &dev_token, &reviewer_token, &pool, "QLE1").await;

    let (status, attempt) = send(app.clone(), Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt:?}");
    let attempt_id = Uuid::parse_str(attempt["attempt_id"].as_str().unwrap()).unwrap();

    let answers: serde_json::Map<String, Value> = (1..=5).map(|n| (n.to_string(), json!("B"))).collect();
    let (status, submitted) = send(app, Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"quiz_answers": answers})).await;
    assert_eq!(status, StatusCode::OK, "{submitted:?}");

    // Exactly one quiz_attempt_submitted, correct shape.
    let submitted_events = sqlx::query!(
        r#"select source, module_item_id, content_version, (payload->>'score')::float8 as score, (payload->>'pending_review')::boolean as pending_review
           from learning_events where event_type = 'quiz_attempt_submitted' and entity_id = $1"#,
        attempt_id,
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(submitted_events.len(), 1, "exactly one quiz_attempt_submitted event per attempt");
    let ev = &submitted_events[0];
    assert_eq!(ev.source, "practice", "no proctoring on this item");
    assert_eq!(ev.module_item_id, Some(item_id));
    assert_eq!(ev.content_version, Some(1), "the item was published exactly once");
    assert_eq!(ev.score, Some(100.0));
    assert_eq!(ev.pending_review, Some(false));

    // Exactly 5 question_answered, one per question, each carrying the
    // question's OWN uid as both entity_id and content_uid.
    let uids: Vec<Uuid> = sqlx::query_scalar!(r#"select q->>'uid' as "uid!" from module_items mi, jsonb_array_elements(mi.quiz_config->'question_groups') g, jsonb_array_elements(g->'questions') q where mi.id = $1"#, item_id)
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|s| Uuid::parse_str(&s).unwrap())
        .collect();
    assert_eq!(uids.len(), 5);

    let answered_events = sqlx::query!(
        r#"select entity_id, content_uid, content_version, source, module_item_id, (payload->>'correct')::boolean as correct
           from learning_events where event_type = 'question_answered' and module_item_id = $1 and entity_id = any($2)"#,
        item_id,
        &uids,
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(answered_events.len(), 5, "one question_answered per question, no more no less");
    for ev in &answered_events {
        assert_eq!(ev.content_uid, Some(ev.entity_id.to_string()), "content_uid must match entity_id — the same question's uid, in both places");
        assert_eq!(ev.content_version, Some(1));
        assert_eq!(ev.source, "practice");
        assert_eq!(ev.module_item_id, Some(item_id));
        assert_eq!(ev.correct, Some(true), "every answer submitted was the correct one (B)");
    }

    // module_item_completed also fired, self_learning is NOT its source
    // here — it's a quiz completion, not an article read.
    let completed: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from learning_events where event_type = 'module_item_completed' and entity_id = $1 and user_id = $2"#, item_id, student_uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(completed, 1);
}

#[sqlx::test]
async fn a_proctored_submission_is_recorded_with_source_tryout(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "qle2-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "qle2-rev@example.com", "reviewer").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "qle2-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let item_id = create_published_quiz(app.clone(), &dev_token, &reviewer_token, &pool, "QLE2").await;

    // Turn proctoring on for this item, and open the sitting a real
    // preflight would have opened — item_proctor::session_for_submit
    // requires both: "enabled" in the effective config, AND an existing
    // quiz_proctor_sessions row with no attempt_id yet, started recently.
    sqlx::query!(r#"update module_items set proctor_config = '{"enabled": true}' where id = $1"#, item_id).execute(&pool).await.unwrap();
    sqlx::query!(r#"insert into quiz_proctor_sessions (user_id, item_id, config, started_at) values ($1, $2, '{}', now())"#, student_uid, item_id).execute(&pool).await.unwrap();

    let (status, attempt) = send(app.clone(), Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt:?}");
    let attempt_id = Uuid::parse_str(attempt["attempt_id"].as_str().unwrap()).unwrap();

    let answers: serde_json::Map<String, Value> = (1..=5).map(|n| (n.to_string(), json!("B"))).collect();
    let (status, submitted) = send(app, Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"quiz_answers": answers})).await;
    assert_eq!(status, StatusCode::OK, "{submitted:?}");

    let sources: Vec<String> = sqlx::query_scalar!(r#"select source from learning_events where user_id = $1 and (entity_id = $2 or module_item_id = $2)"#, student_uid, item_id)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(!sources.is_empty());
    assert!(sources.iter().all(|s| s == "tryout"), "every event from a proctored sitting must be source=tryout, got: {sources:?}");
}

#[sqlx::test]
async fn grading_a_manual_question_writes_a_question_graded_event_attributed_to_the_student(pool: PgPool) {
    let (_dev_uid, dev_token) = insert_user_with_role(&pool, "qle3-dev@example.com", "curriculum_developer").await;
    let (_rev_uid, reviewer_token) = insert_user_with_role(&pool, "qle3-rev@example.com", "reviewer").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "qle3-student@example.com", "student").await;
    // `Attempt`/`Action::Submit` — which `grade_manual_group` itself
    // gates on — only permits `student`/`platform_admin` today (see
    // permissions.rs); a `platform_admin` grader sidesteps that
    // pre-existing, unrelated permission-matrix quirk rather than
    // exercising it.
    let (_grader_uid, grader_token) = insert_user_with_role(&pool, "qle3-grader@example.com", "platform_admin").await;
    let app = build_app(pool.clone());

    // A single Manual-graded (file_upload) question — a teacher, not
    // the auto-grader, decides its score.
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('SUBJ-QLE3', 'SUBJ-QLE3') returning id"#).fetch_one(&pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": "MOD-QLE3", "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();
    let (status, item) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "item", "title": "Quiz QLE3", "content_type": "quiz"})).await;
    assert_eq!(status, StatusCode::CREATED, "{item:?}");
    let item_id = Uuid::parse_str(item["id"].as_str().unwrap()).unwrap();

    let quiz_config = json!({"question_groups": [{"group_id": "g1", "type": "file_upload", "questions": [{"number": 1, "prompt": "Upload jawabanmu"}]}]});
    let (status, _) = send(app.clone(), Method::PATCH, &format!("/module-items/{item_id}/quiz-config"), &dev_token, json!({"quiz_config": quiz_config})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/submit-review"), &dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/publish"), &reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let expected_uid: Uuid = Uuid::parse_str(
        &sqlx::query_scalar!(r#"select q->>'uid' as "uid!" from module_items mi, jsonb_array_elements(mi.quiz_config->'question_groups') g, jsonb_array_elements(g->'questions') q where mi.id = $1"#, item_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
    )
    .unwrap();

    let (status, attempt) = send(app.clone(), Method::POST, &format!("/lessons/{item_id}/attempts"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{attempt:?}");
    let attempt_id = attempt["attempt_id"].as_str().unwrap().to_string();
    let (status, submitted) = send(app.clone(), Method::POST, &format!("/attempts/{attempt_id}/submit"), &student_token, json!({"quiz_answers": {"1": {"asset_id": null}}})).await;
    assert_eq!(status, StatusCode::OK, "{submitted:?}");
    assert_eq!(submitted["status"], json!("submitted"), "a Manual question leaves the attempt pending review");

    let (status, _) = send(app, Method::POST, &format!("/attempts/{attempt_id}/grade"), &grader_token, json!({"group_id": "g1", "question_number": "1", "score": 75.0})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let row = sqlx::query!(
        r#"select user_id, content_uid, source, (payload->>'score')::float8 as score from learning_events where event_type = 'question_graded' and entity_id = $1"#,
        expected_uid,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.user_id, student_uid, "the event belongs to the STUDENT whose answer was graded, not the teacher who graded it");
    assert_eq!(row.content_uid, Some(expected_uid.to_string()));
    assert_eq!(row.source, "practice");
    assert_eq!(row.score, Some(75.0));
}
