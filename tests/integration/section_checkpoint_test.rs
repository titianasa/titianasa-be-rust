// Section checkpoints (services/section_checkpoint.rs): server-graded,
// sequential, a wrong answer forces a timed re-read, and passing them all
// completes the article so its Latihan unlocks.

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



fn checkpoint_pool(tag: &str) -> Value {
    let questions: Vec<Value> = (1..=3)
        .map(|i| {
            json!({
                "number": i,
                "stem": format!("Checkpoint {tag} soal {i}"),
                "choices": [{"label": "A", "text": format!("benar {tag}{i}")}, {"label": "B", "text": format!("keliru {tag}{i}")}, {"label": "C", "text": format!("salah {tag}{i}")}],
                "answer": "A",
                "explanation": format!("Karena {tag}{i}"),
                "taxonomy": {"bloom": "c1", "difficulty": "mudah"}
            })
        })
        .collect();
    json!({"question_groups": [{"group_id": "cp", "type": "multiple_choice", "questions": questions}]})
}

fn right_answers(paper: &Value) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for group in paper["question_groups"].as_array().unwrap() {
        for q in group["questions"].as_array().unwrap() {
            let label = q["choices"].as_array().unwrap().iter().find(|c| c["text"].as_str().unwrap().starts_with("benar")).unwrap()["label"].clone();
            out.insert(q["number"].to_string(), label);
        }
    }
    out
}

fn wrong_answers(paper: &Value) -> serde_json::Map<String, Value> {
    right_answers(paper).into_iter().map(|(k, v)| (k, json!(if v == "A" { "B" } else { "A" }))).collect()
}

#[sqlx::test]
async fn checkpoints_gate_each_section_force_a_reread_on_failure_and_unlock_the_latihan(pool: PgPool) {
    let (_, dev_token) = insert_user_with_role(&pool, "cp-dev@example.com", "curriculum_developer").await;
    let (_, reviewer_token) = insert_user_with_role(&pool, "cp-rev@example.com", "reviewer").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "cp-student@example.com", "student").await;
    let app = build_app(pool.clone());

    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('SUBJ-CP', 'SUBJ-CP') returning id"#).fetch_one(&pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": "MOD-CP", "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();

    let (_, article) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "item", "title": "Pembahasan", "content_type": "article"})).await;
    let article_id = article["id"].as_str().unwrap().to_string();
    let plan = json!({"title": "Modul", "level": "Tahap 1", "sections": [
        {"id": "sec1", "title": "Bagian 1", "minutes": 2, "content": "Isi bagian satu.", "checkpoint": checkpoint_pool("x")},
        {"id": "sec2", "title": "Bagian 2", "minutes": 2, "content": "Isi bagian dua.", "checkpoint": checkpoint_pool("y")}
    ]});
    let (status, body) = send(app.clone(), Method::PATCH, &format!("/module-items/{article_id}/lesson-plan"), &dev_token, json!({"lesson_plan": plan})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let (status, body) = send(app.clone(), Method::PATCH, &format!("/module-items/{article_id}/guards"), &dev_token, json!({"guard_config": {"completion_rule": "required"}})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let (_, quiz) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "item", "title": "Latihan", "content_type": "quiz"})).await;
    let quiz_id = quiz["id"].as_str().unwrap().to_string();
    for id in [&article_id, &quiz_id] {
        let (status, body) = send(app.clone(), Method::POST, &format!("/module-items/{id}/submit-review"), &dev_token, json!({})).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        let (status, body) = send(app.clone(), Method::POST, &format!("/module-items/{id}/publish"), &reviewer_token, json!({})).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
    }

    // The pool never reaches the learner.
    let (_, detail) = send(app.clone(), Method::GET, &format!("/module-items/{article_id}"), &student_token, Value::Null).await;
    assert!(!detail["lesson_plan"].to_string().contains("checkpoint"), "{detail:?}");
    // Latihan is locked behind the article.
    let (status, _) = send(app.clone(), Method::GET, &format!("/module-items/{quiz_id}"), &student_token, Value::Null).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, cps) = send(app.clone(), Method::GET, &format!("/module-items/{article_id}/checkpoints"), &student_token, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{cps:?}");
    assert_eq!(cps["sections"][0]["status"], "open");
    assert_eq!(cps["sections"][1]["status"], "locked");
    let paper1 = cps["sections"][0]["paper"].clone();
    assert_eq!(paper1["question_groups"][0]["questions"].as_array().unwrap().len(), 2, "draws 2 of the pool's 3");
    assert!(!paper1.to_string().contains("\"answer\""));

    // Refresh keeps the same draw.
    let (_, again) = send(app.clone(), Method::GET, &format!("/module-items/{article_id}/checkpoints"), &student_token, Value::Null).await;
    assert_eq!(again["sections"][0]["paper"], paper1);

    // Out of order, and skipping via "Selesai", are both refused.
    let (status, body) = send(app.clone(), Method::POST, &format!("/module-items/{article_id}/sections/sec2/checkpoint"), &student_token, json!({"answers": {}})).await;
    assert_eq!((status, body["error"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("checkpoint_locked")));
    let (status, body) = send(app.clone(), Method::POST, &format!("/module-items/{article_id}/complete"), &student_token, json!({})).await;
    assert_eq!((status, body["error"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("checkpoints_incomplete")));

    // Wrong → locked for re-reading; an immediate retry is refused.
    let (status, failed) = send(app.clone(), Method::POST, &format!("/module-items/{article_id}/sections/sec1/checkpoint"), &student_token, json!({"answers": wrong_answers(&paper1)})).await;
    assert_eq!(status, StatusCode::OK, "{failed:?}");
    assert_eq!(failed["passed"], false);
    assert!(failed["reread_ready_at"].is_string());
    assert!(failed["results"][0]["explanation"].as_str().unwrap().starts_with("Karena"));
    let (status, body) = send(app.clone(), Method::POST, &format!("/module-items/{article_id}/sections/sec1/reread"), &student_token, json!({})).await;
    assert_eq!((status, body["error"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("reread_too_soon")));

    // After the minimum re-read time, a fresh draw.
    sqlx::query!(r#"update section_checkpoint_progress set locked_at = now() - interval '10 minutes' where user_id = $1"#, student_uid).execute(&pool).await.unwrap();
    let (status, reopened) = send(app.clone(), Method::POST, &format!("/module-items/{article_id}/sections/sec1/reread"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{reopened:?}");
    assert_eq!(reopened["sections"][0]["status"], "open");
    let paper1b = reopened["sections"][0]["paper"].clone();

    let (_, passed1) = send(app.clone(), Method::POST, &format!("/module-items/{article_id}/sections/sec1/checkpoint"), &student_token, json!({"answers": right_answers(&paper1b)})).await;
    assert_eq!(passed1["passed"], true, "{passed1:?}");
    assert_eq!(passed1["xp_awarded"], 4);
    assert_eq!(passed1["all_passed"], false);

    let (_, cps) = send(app.clone(), Method::GET, &format!("/module-items/{article_id}/checkpoints"), &student_token, Value::Null).await;
    assert_eq!(cps["sections"][0]["status"], "passed");
    assert_eq!(cps["sections"][1]["status"], "open");
    let (_, passed2) = send(app.clone(), Method::POST, &format!("/module-items/{article_id}/sections/sec2/checkpoint"), &student_token, json!({"answers": right_answers(&cps["sections"][1]["paper"])})).await;
    assert_eq!(passed2["all_passed"], true, "{passed2:?}");

    // Article complete → the Latihan opens.
    let (status, body) = send(app.clone(), Method::GET, &format!("/module-items/{quiz_id}"), &student_token, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
}

#[sqlx::test]
async fn an_author_previews_checkpoints_on_a_draft_without_leaving_any_progress(pool: PgPool) {
    // /belajar/{id}/preview: the same draw and grader as a learner, on a
    // draft, with nothing stored — sections never lock, nothing pays.
    let (dev_uid, dev_token) = insert_user_with_role(&pool, "cpp-dev@example.com", "curriculum_developer").await;
    let (_, student_token) = insert_user_with_role(&pool, "cpp-student@example.com", "student").await;
    let app = build_app(pool.clone());
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('SUBJ-CPP', 'SUBJ-CPP') returning id"#).fetch_one(&pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": "MOD-CPP", "title": "Module"})).await;
    let (_, article) = send(app.clone(), Method::POST, &format!("/modules/{}/items", module["id"].as_str().unwrap()), &dev_token, json!({"node_type": "item", "title": "Pembahasan", "content_type": "article"})).await;
    let article_id = article["id"].as_str().unwrap().to_string();
    let plan = json!({"title": "Modul", "level": "Tahap 1", "sections": [
        {"id": "sec1", "title": "Bagian 1", "minutes": 2, "content": "Isi.", "checkpoint": checkpoint_pool("x")},
        {"id": "sec2", "title": "Bagian 2", "minutes": 2, "content": "Isi.", "checkpoint": checkpoint_pool("y")}
    ]});
    let (status, body) = send(app.clone(), Method::PATCH, &format!("/module-items/{article_id}/lesson-plan"), &dev_token, json!({"lesson_plan": plan})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let preview = |section: &str| format!("/module-items/{article_id}/sections/{section}/checkpoint/preview");
    let (status, denied) = send(app.clone(), Method::POST, &preview("sec1"), &student_token, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied:?}");

    // Section 2 straight away — nothing is locked in a preview.
    let (status, drawn) = send(app.clone(), Method::POST, &preview("sec2"), &dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{drawn:?}");
    assert_eq!(drawn["paper"]["question_groups"][0]["questions"].as_array().unwrap().len(), 2);

    let grade = |answers: serde_json::Map<String, Value>, draw: &Value| json!({"draw": draw, "answers": answers});
    let (status, wrong) = send(app.clone(), Method::POST, &format!("{}/grade", preview("sec2")), &dev_token, grade(wrong_answers(&drawn["paper"]), &drawn["draw"])).await;
    assert_eq!(status, StatusCode::OK, "{wrong:?}");
    assert_eq!(wrong["passed"], false);
    assert!(wrong["results"][0]["explanation"].is_string());
    let (_, right) = send(app.clone(), Method::POST, &format!("{}/grade", preview("sec2")), &dev_token, grade(right_answers(&drawn["paper"]), &drawn["draw"])).await;
    assert_eq!(right["passed"], true, "shuffled, re-lettered answers grade right: {right:?}");

    // "Soal lain": a new draw that avoids the previous one where it can.
    let (_, next) = send(app.clone(), Method::POST, &preview("sec2"), &dev_token, json!({"previous": drawn["draw"]})).await;
    assert_eq!(next["paper"]["question_groups"][0]["questions"].as_array().unwrap().len(), 2);

    let stored: i64 = sqlx::query_scalar!(r#"select count(*) as "n!" from section_checkpoint_progress where user_id = $1"#, dev_uid).fetch_one(&pool).await.unwrap();
    let xp: i64 = sqlx::query_scalar!(r#"select coalesce(sum(amount), 0)::bigint as "t!" from xp_events where user_id = $1"#, dev_uid).fetch_one(&pool).await.unwrap();
    let events: i64 = sqlx::query_scalar!(r#"select count(*) as "n!" from learning_events where user_id = $1"#, dev_uid).fetch_one(&pool).await.unwrap();
    assert_eq!((stored, xp, events), (0, 0, 0), "a preview leaves no progress, XP or learning events");
}
