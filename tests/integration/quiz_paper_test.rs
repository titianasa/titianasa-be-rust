// Server-built quiz papers (services/quiz_paper.rs): answer keys stay on
// the server, pooled quizzes draw and shuffle per attempt, grading maps
// displayed letters back, and quiz XP can't be farmed by retaking.

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
    // 204 (a delete) has no body.
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


fn bank_questions(n: usize) -> Value {
    let questions: Vec<Value> = (1..=n)
        .map(|i| {
            json!({
                "number": i,
                "stem": format!("Soal bank nomor {i} tentang bilangan"),
                "choices": [
                    {"label": "A", "text": format!("benar {i}")},
                    {"label": "B", "text": format!("pengecoh satu {i}")},
                    {"label": "C", "text": format!("pengecoh dua {i}")},
                    {"label": "D", "text": format!("pengecoh tiga {i}")}
                ],
                "answer": "A",
                "explanation": format!("Pembahasan {i}"),
                "taxonomy": {"bloom": if i % 2 == 0 { "c2" } else { "c1" }, "difficulty": "mudah"}
            })
        })
        .collect();
    json!([{"group_id": "mc", "type": "multiple_choice", "instruction": "Pilih jawaban yang tepat.", "questions": questions}])
}

struct Fixture {
    app: axum::Router,
    dev_token: String,
    student_token: String,
    student_uid: Uuid,
    module_id: String,
    bank_id: String,
    pooled_id: String,
}

async fn publish(app: &axum::Router, dev_token: &str, reviewer_token: &str, item_id: &str) {
    let (status, body) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/submit-review"), dev_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let (status, body) = send(app.clone(), Method::POST, &format!("/module-items/{item_id}/publish"), reviewer_token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
}

async fn create_quiz(app: &axum::Router, dev_token: &str, module_id: &str, title: &str, quiz_config: Value) -> String {
    let (status, item) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), dev_token, json!({"node_type": "item", "title": title, "content_type": "quiz"})).await;
    assert_eq!(status, StatusCode::CREATED, "{item:?}");
    let item_id = item["id"].as_str().unwrap().to_string();
    let (status, body) = send(app.clone(), Method::PATCH, &format!("/module-items/{item_id}/quiz-config"), dev_token, json!({"quiz_config": quiz_config})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    item_id
}

async fn fixture(pool: &PgPool, label: &str, bank_size: usize, draw: i64) -> Fixture {
    let (_, dev_token) = insert_user_with_role(pool, &format!("{label}-dev@example.com"), "curriculum_developer").await;
    let (_, reviewer_token) = insert_user_with_role(pool, &format!("{label}-rev@example.com"), "reviewer").await;
    let (student_uid, student_token) = insert_user_with_role(pool, &format!("{label}-student@example.com"), "student").await;
    let app = build_app(pool.clone());

    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ($1, $1) returning id"#, format!("SUBJ-{label}")).fetch_one(pool).await.unwrap();
    let (_, module) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": false, "subject_id": subject_id, "code": format!("MOD-{label}"), "title": "Module"})).await;
    let module_id = module["id"].as_str().unwrap().to_string();

    let bank_id = create_quiz(&app, &dev_token, &module_id, "Latihan 50 soal", json!({"question_groups": bank_questions(bank_size)})).await;
    let pooled_id = create_quiz(
        &app,
        &dev_token,
        &module_id,
        "Latihan 10 soal",
        json!({"question_groups": [], "passing_score": 70, "question_pool": {"source_item_id": bank_id, "draw_count": draw}}),
    )
    .await;
    publish(&app, &dev_token, &reviewer_token, &bank_id).await;
    publish(&app, &dev_token, &reviewer_token, &pooled_id).await;
    Fixture { app, dev_token, student_token, student_uid, module_id, bank_id, pooled_id }
}

/// The shown letter of the choice whose text starts with "benar".
fn correct_answers(paper: &Value) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for group in paper["question_groups"].as_array().unwrap() {
        for q in group["questions"].as_array().unwrap() {
            let label = q["choices"].as_array().unwrap().iter().find(|c| c["text"].as_str().unwrap().starts_with("benar")).unwrap()["label"].clone();
            out.insert(q["number"].to_string(), label);
        }
    }
    out
}

fn paper_uids(paper: &Value) -> Vec<String> {
    paper["question_groups"].as_array().unwrap().iter().flat_map(|g| g["questions"].as_array().unwrap().iter().map(|q| q["uid"].as_str().unwrap().to_string())).collect()
}

#[sqlx::test]
async fn a_learner_never_receives_the_answer_key_but_the_author_does(pool: PgPool) {
    let f = fixture(&pool, "leak", 6, 3).await;

    let (status, learner) = send(f.app.clone(), Method::GET, &format!("/module-items/{}", f.bank_id), &f.student_token, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{learner:?}");
    let text = learner["quiz_config"].to_string();
    assert!(!text.contains("\"answer\"") && !text.contains("Pembahasan"), "answer key reached a learner: {text}");

    let (_, author) = send(f.app.clone(), Method::GET, &format!("/module-items/{}", f.bank_id), &f.dev_token, Value::Null).await;
    assert_eq!(author["quiz_config"]["question_groups"][0]["questions"][0]["answer"], "A");
}

#[sqlx::test]
async fn a_pooled_quiz_draws_a_shuffled_paper_and_grades_only_that_paper(pool: PgPool) {
    let f = fixture(&pool, "draw", 20, 5).await;

    let (status, started) = send(f.app.clone(), Method::POST, &format!("/lessons/{}/attempts", f.pooled_id), &f.student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{started:?}");
    let paper = &started["paper"];
    assert_eq!(paper_uids(paper).len(), 5, "draw_count 5 from a bank of 20");
    assert!(!paper.to_string().contains("\"answer\""), "the paper itself carries no key");
    let numbers: Vec<String> = paper["question_groups"][0]["questions"].as_array().unwrap().iter().map(|q| q["number"].to_string()).collect();
    assert_eq!(numbers, vec!["1", "2", "3", "4", "5"], "renumbered in paper order");

    // Refresh / second tab: same attempt, identical paper — no re-roll.
    let (status, again) = send(f.app.clone(), Method::POST, &format!("/lessons/{}/attempts", f.pooled_id), &f.student_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{again:?}");
    assert_eq!(again["attempt_id"], started["attempt_id"]);
    assert_eq!(&again["paper"], paper);

    let attempt_id = started["attempt_id"].as_str().unwrap();
    let (status, result) = send(f.app.clone(), Method::POST, &format!("/attempts/{attempt_id}/submit"), &f.student_token, json!({"quiz_answers": correct_answers(paper)})).await;
    assert_eq!(status, StatusCode::OK, "{result:?}");
    assert_eq!(result["score"], 100.0, "answers in displayed letters must grade as correct: {result:?}");
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 5, "the 15 undrawn bank questions are not graded");
    assert!(results[0]["explanation"].as_str().unwrap().starts_with("Pembahasan"));
    assert!(results.iter().all(|r| r["correct_answer"].is_string()));
}

#[sqlx::test]
async fn wrong_letters_grade_wrong_and_a_retake_draws_new_questions(pool: PgPool) {
    let f = fixture(&pool, "retake", 10, 5).await;

    let (_, first) = send(f.app.clone(), Method::POST, &format!("/lessons/{}/attempts", f.pooled_id), &f.student_token, json!({})).await;
    let first_paper = first["paper"].clone();
    // Pick a letter that is NOT the correct one for every question.
    let wrong: serde_json::Map<String, Value> = correct_answers(&first_paper)
        .into_iter()
        .map(|(k, v)| (k, json!(if v == "A" { "B" } else { "A" })))
        .collect();
    let (_, result) = send(f.app.clone(), Method::POST, &format!("/attempts/{}/submit", first["attempt_id"].as_str().unwrap()), &f.student_token, json!({"quiz_answers": wrong})).await;
    assert_eq!(result["score"], 0.0, "{result:?}");

    let (_, second) = send(f.app.clone(), Method::POST, &format!("/lessons/{}/attempts", f.pooled_id), &f.student_token, json!({})).await;
    assert_ne!(second["attempt_id"], first["attempt_id"]);
    let seen: std::collections::HashSet<String> = paper_uids(&first_paper).into_iter().collect();
    let overlap = paper_uids(&second["paper"]).iter().filter(|u| seen.contains(*u)).count();
    assert_eq!(overlap, 0, "10-question bank, 5 seen — the retake should be all new");
}

#[sqlx::test]
async fn quiz_xp_is_paid_once_on_first_pass_and_not_farmed_by_retakes(pool: PgPool) {
    let f = fixture(&pool, "xpfarm", 20, 5).await;
    let xp_total = |pool: PgPool, uid: Uuid| async move { sqlx::query_scalar!(r#"select coalesce(sum(amount), 0)::bigint as "t!" from xp_events where user_id = $1 and reason like 'quiz_%'"#, uid).fetch_one(&pool).await.unwrap() };

    for round in 0..3 {
        let (_, started) = send(f.app.clone(), Method::POST, &format!("/lessons/{}/attempts", f.pooled_id), &f.student_token, json!({})).await;
        let answers = correct_answers(&started["paper"]);
        let (status, result) = send(f.app.clone(), Method::POST, &format!("/attempts/{}/submit", started["attempt_id"].as_str().unwrap()), &f.student_token, json!({"quiz_answers": answers})).await;
        assert_eq!(status, StatusCode::OK, "round {round}: {result:?}");
    }
    assert_eq!(xp_total(pool.clone(), f.student_uid).await, 10, "5-question paper = 2×5 XP, once — two perfect retakes add nothing");
}

#[sqlx::test]
async fn a_pool_must_point_at_a_quiz_in_the_same_module(pool: PgPool) {
    let f = fixture(&pool, "poolcheck", 4, 2).await;
    let other_subject: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('SUBJ-OTHER', 'SUBJ-OTHER') returning id"#).fetch_one(&pool).await.unwrap();
    let (_, other_module) = send(f.app.clone(), Method::POST, "/modules", &f.dev_token, json!({"is_folder": false, "subject_id": other_subject, "code": "MOD-OTHER", "title": "Other"})).await;
    let (_, item) = send(f.app.clone(), Method::POST, &format!("/modules/{}/items", other_module["id"].as_str().unwrap()), &f.dev_token, json!({"node_type": "item", "title": "Asing", "content_type": "quiz"})).await;
    let (status, body) = send(
        f.app.clone(),
        Method::PATCH,
        &format!("/module-items/{}/quiz-config", item["id"].as_str().unwrap()),
        &f.dev_token,
        json!({"quiz_config": {"question_groups": [], "question_pool": {"source_item_id": f.bank_id, "draw_count": 2}}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:?}");
    assert_eq!(body["error"], "invalid_question_pool");
    let _ = &f.module_id;
}

#[sqlx::test]
async fn the_preview_door_is_separate_from_the_learner_door_and_leaves_no_history(pool: PgPool) {
    // /belajar/{id}/preview sits a quiz with `?mode=preview`. Reviewing a
    // generated bank used to be impossible: "Mulai Kuis" answered
    // `lesson_not_published` for a draft and a bare `forbidden` for an
    // author on a published quiz.
    let f = fixture(&pool, "preview", 12, 4).await;
    let draft_id = create_quiz(&f.app, &f.dev_token, &f.module_id, "Latihan 1 — Draf", json!({"question_groups": bank_questions(6), "passing_score": 70})).await;
    let author_uid: Uuid = sqlx::query_scalar!(r#"select id from users where email = 'preview-dev@example.com'"#).fetch_one(&pool).await.unwrap();

    // Learner door: a learner on a draft, an author at all.
    let (status, denied) = send(f.app.clone(), Method::POST, &format!("/lessons/{draft_id}/attempts"), &f.student_token, json!({})).await;
    assert_eq!((status, denied["error"].as_str()), (StatusCode::FORBIDDEN, Some("lesson_not_published")));
    let (status, _) = send(f.app.clone(), Method::POST, &format!("/lessons/{draft_id}/attempts"), &f.dev_token, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the learner door is never a silent preview");
    // Preview door: not for a learner.
    let (status, denied) = send(f.app.clone(), Method::POST, &format!("/lessons/{draft_id}/attempts?mode=preview"), &f.student_token, json!({})).await;
    assert_eq!((status, denied["error"].as_str()), (StatusCode::FORBIDDEN, Some("preview_not_allowed")));

    // Author previews the draft: real paper, blind, really graded.
    let (status, started) = send(f.app.clone(), Method::POST, &format!("/lessons/{draft_id}/attempts?mode=preview"), &f.dev_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{started:?}");
    assert_eq!(started["preview"], true);
    let paper = started["paper"].clone();
    assert_eq!(paper_uids(&paper).len(), 6);
    assert!(!paper.to_string().contains("\"answer\""), "a preview is sat blind: {paper}");
    let (status, result) = send(f.app.clone(), Method::POST, &format!("/attempts/{}/submit", started["attempt_id"].as_str().unwrap()), &f.dev_token, json!({"quiz_answers": correct_answers(&paper)})).await;
    assert_eq!(status, StatusCode::OK, "{result:?}");
    assert_eq!(result["score"], 100.0);

    // Author previews the PUBLISHED pooled quiz too.
    let (status, started) = send(f.app.clone(), Method::POST, &format!("/lessons/{}/attempts?mode=preview", f.pooled_id), &f.dev_token, json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{started:?}");
    assert_eq!(paper_uids(&started["paper"]).len(), 4, "draws from the bank like a learner's paper");
    let (status, _) = send(f.app.clone(), Method::POST, &format!("/attempts/{}/submit", started["attempt_id"].as_str().unwrap()), &f.dev_token, json!({"quiz_answers": correct_answers(&started["paper"])})).await;
    assert_eq!(status, StatusCode::OK);

    // Nothing counted: no XP, no completion, no learning events.
    let xp: i64 = sqlx::query_scalar!(r#"select coalesce(sum(amount), 0)::bigint as "t!" from xp_events where user_id = $1"#, author_uid).fetch_one(&pool).await.unwrap();
    assert_eq!(xp, 0);
    let progress: i64 = sqlx::query_scalar!(r#"select count(*) as "n!" from module_item_progress where user_id = $1"#, author_uid).fetch_one(&pool).await.unwrap();
    assert_eq!(progress, 0, "a preview never completes an item");
    let events: i64 = sqlx::query_scalar!(r#"select count(*) as "n!" from learning_events where user_id = $1"#, author_uid).fetch_one(&pool).await.unwrap();
    assert_eq!(events, 0, "a preview never feeds the learning engine");

    // A previewed draft can still be deleted.
    let (status, body) = send(f.app.clone(), Method::DELETE, &format!("/module-items/{draft_id}"), &f.dev_token, Value::Null).await;
    assert!(status.is_success(), "{status} {body:?}");

    // And a learner on the published quiz is a real sitting.
    let (_, learner) = send(f.app.clone(), Method::POST, &format!("/lessons/{}/attempts", f.pooled_id), &f.student_token, json!({})).await;
    assert!(learner.get("preview").is_none(), "{learner:?}");
}

#[sqlx::test]
async fn reports_on_the_same_question_collect_into_one_ticket_that_an_admin_works(pool: PgPool) {
    let f = fixture(&pool, "report", 8, 4).await;
    let (_, student2_token) = insert_user_with_role(&pool, "report-student2@example.com", "student").await;
    let (_, admin_token) = insert_user_with_role(&pool, "report-admin@example.com", "platform_admin").await;

    // A learner in the POOLED Latihan flags a question from their paper.
    let (_, started) = send(f.app.clone(), Method::POST, &format!("/lessons/{}/attempts", f.pooled_id), &f.student_token, json!({})).await;
    let q = started["paper"]["question_groups"][0]["questions"][0].clone();
    let uid = q["uid"].as_str().unwrap().to_string();
    let report = |category: &str, message: Option<&str>| {
        json!({"module_item_id": f.pooled_id, "target_type": "question", "content_uid": uid, "category": category, "message": message,
               "context": {"surface": "quiz", "shown_number": q["number"], "choices": q["choices"]}})
    };
    let (status, body) = send(f.app.clone(), Method::POST, "/content-reports", &f.student_token, report("wrong_answer_key", Some("Kuncinya harusnya B"))).await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    // Flagging again updates their report; a second learner adds a vote.
    send(f.app.clone(), Method::POST, "/content-reports", &f.student_token, report("bad_choices", None)).await;
    let (status, _) = send(f.app.clone(), Method::POST, "/content-reports", &student2_token, report("typo", None)).await;
    assert_eq!(status, StatusCode::CREATED);

    // Garbage is refused.
    let (status, body) = send(f.app.clone(), Method::POST, "/content-reports", &f.student_token, json!({"module_item_id": f.pooled_id, "target_type": "question", "content_uid": "not-a-question", "category": "typo"})).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("content_not_found")));
    let (status, body) = send(f.app.clone(), Method::POST, "/content-reports", &f.student_token, report("other", None)).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("message_required")));

    // Learners never see the queue.
    let (status, _) = send(f.app.clone(), Method::GET, "/admin/content-tickets", &f.student_token, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, list) = send(f.app.clone(), Method::GET, "/admin/content-tickets?status=open", &admin_token, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{list:?}");
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "one ticket for one question: {list:?}");
    let ticket = &items[0];
    assert_eq!(ticket["report_count"], 2, "two people, not three submissions");
    assert_eq!(ticket["module_item_id"], f.bank_id.as_str(), "the ticket points at the BANK, where the fix is made");
    assert_eq!(ticket["categories"], json!(["bad_choices", "typo"]));
    assert!(ticket["summary"].as_str().unwrap().starts_with("Soal bank nomor"));
    assert_eq!(ticket["context"]["subject_name"], "SUBJ-report");

    let ticket_id = ticket["id"].as_str().unwrap();
    let (_, detail) = send(f.app.clone(), Method::GET, &format!("/admin/content-tickets/{ticket_id}"), &admin_token, Value::Null).await;
    assert_eq!(detail["content"]["question"]["answer"], "A", "the admin sees the key");
    assert_eq!(detail["reports"].as_array().unwrap().len(), 2);
    assert!(detail["reports"][0].get("reporter_id").is_none(), "no learner identity on the ticket");

    let (status, body) = send(f.app.clone(), Method::PATCH, &format!("/admin/content-tickets/{ticket_id}"), &admin_token, json!({"status": "rejected"})).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("note_required")));
    let (status, body) = send(f.app.clone(), Method::PATCH, &format!("/admin/content-tickets/{ticket_id}"), &admin_token, json!({"status": "resolved", "resolution_note": "Pilihan B diperjelas"})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["ticket"]["status"], "resolved");
    let audited: i64 = sqlx::query_scalar!(r#"select count(*) as "n!" from admin_audit_log where action = 'content_ticket.updated'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(audited, 1);

    // A new report after the fix opens a fresh ticket.
    send(f.app.clone(), Method::POST, "/content-reports", &student2_token, report("unclear", None)).await;
    let (_, open) = send(f.app.clone(), Method::GET, "/admin/content-tickets?status=open", &admin_token, Value::Null).await;
    assert_eq!(open["items"].as_array().unwrap().len(), 1);
    assert_ne!(open["items"][0]["id"], ticket["id"]);
}

#[sqlx::test]
async fn graded_answers_become_facts_that_heatmaps_drill_through(pool: PgPool) {
    let (dev_uid, dev_token) = insert_user_with_role(&pool, "heat-dev@example.com", "curriculum_developer").await;
    let (_, reviewer_token) = insert_user_with_role(&pool, "heat-rev@example.com", "reviewer").await;
    let (_, admin_token) = insert_user_with_role(&pool, "heat-admin@example.com", "platform_admin").await;
    let (student_uid, student_token) = insert_user_with_role(&pool, "heat-student@example.com", "student").await;
    let (teacher_uid, teacher_token) = insert_user_with_role(&pool, "heat-teacher@example.com", "teacher").await;
    let (_, other_teacher_token) = insert_user_with_role(&pool, "heat-teacher2@example.com", "teacher").await;
    let (outsider_uid, _) = insert_user_with_role(&pool, "heat-outsider@example.com", "student").await;
    let app = build_app(pool.clone());

    // Matematika › Tahap 1 › Topik › Bab A › Latihan
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('MAT-H', 'Matematika') returning id"#).fetch_one(&pool).await.unwrap();
    let (_, subject_folder) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": true, "title": "Matematika"})).await;
    let (_, tahap) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": true, "title": "Tahap 1", "parent_id": subject_folder["id"]})).await;
    let (status, topik) = send(app.clone(), Method::POST, "/modules", &dev_token, json!({"is_folder": false, "title": "Bilangan Cacah", "code": "MOD-HEAT", "subject_id": subject_id, "parent_id": tahap["id"]})).await;
    assert_eq!(status, StatusCode::CREATED, "{topik:?}");
    let module_id = topik["id"].as_str().unwrap().to_string();
    let (_, bab) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "section", "title": "Bab A"})).await;
    let (status, quiz) = send(app.clone(), Method::POST, &format!("/modules/{module_id}/items"), &dev_token, json!({"node_type": "item", "title": "Latihan 1 — Bab A", "content_type": "quiz", "parent_id": bab["id"]})).await;
    assert_eq!(status, StatusCode::CREATED, "{quiz:?}");
    let quiz_id = quiz["id"].as_str().unwrap().to_string();
    let (status, body) = send(app.clone(), Method::PATCH, &format!("/module-items/{quiz_id}/quiz-config"), &dev_token, json!({"quiz_config": {"question_groups": bank_questions(6), "passing_score": 70}})).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    publish(&app, &dev_token, &reviewer_token, &quiz_id).await;

    // The student gets 5 of 6 right.
    let (_, started) = send(app.clone(), Method::POST, &format!("/lessons/{quiz_id}/attempts"), &student_token, json!({})).await;
    let mut answers = correct_answers(&started["paper"]);
    let first = answers.keys().next().unwrap().clone();
    answers.insert(first, json!("Z"));
    let (status, _) = send(app.clone(), Method::POST, &format!("/attempts/{}/submit", started["attempt_id"].as_str().unwrap()), &student_token, json!({"quiz_answers": answers})).await;
    assert_eq!(status, StatusCode::OK);
    // An author's preview is not a fact.
    let (_, preview) = send(app.clone(), Method::POST, &format!("/lessons/{quiz_id}/attempts?mode=preview"), &dev_token, json!({})).await;
    send(app.clone(), Method::POST, &format!("/attempts/{}/submit", preview["attempt_id"].as_str().unwrap()), &dev_token, json!({"quiz_answers": correct_answers(&preview["paper"])})).await;

    let facts = sqlx::query!(r#"select user_id, folder_path, bab_id, module_id, subject_id, difficulty, bloom, correct, source from question_answer_facts"#).fetch_all(&pool).await.unwrap();
    assert_eq!(facts.len(), 6, "six graded answers from the student, none from the preview");
    assert!(facts.iter().all(|f| f.user_id == student_uid && f.user_id != dev_uid));
    let f = &facts[0];
    assert_eq!(f.folder_path.len(), 3, "Matematika › Tahap 1 › topik");
    assert_eq!(f.bab_id.map(|b| b.to_string()), bab["id"].as_str().map(str::to_string));
    assert_eq!((f.subject_id, f.difficulty.as_deref(), f.source.as_str()), (Some(subject_id), Some("mudah"), "practice"));
    assert_eq!(facts.iter().filter(|f| f.correct == Some(true)).count(), 5);

    // The student's own map drills straight past single-child levels to the bab.
    let (status, mine) = send(app.clone(), Method::GET, "/me/learning-heatmap?axis=bloom", &student_token, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{mine:?}");
    let crumbs: Vec<&str> = mine["breadcrumb"].as_array().unwrap().iter().map(|c| c["title"].as_str().unwrap()).collect();
    assert_eq!(crumbs, vec!["Matematika", "Tahap 1", "Bilangan Cacah"]);
    assert_eq!(mine["rows"].as_array().unwrap().len(), 1);
    assert_eq!(mine["rows"][0]["title"], "Bab A");
    assert_eq!(mine["rows"][0]["kind"], "bab");
    assert_eq!((mine["total_graded"].as_i64(), mine["total_correct"].as_i64()), (Some(6), Some(5)));
    let c1 = mine["rows"][0]["cells"].as_array().unwrap().iter().find(|c| c["key"] == "c1").unwrap();
    assert_eq!(c1["graded"], 3, "bank_questions alternates c1/c2");

    // Admin Pusat reads the rollup, never the facts, so nothing until it runs.
    let (_, before) = send(app.clone(), Method::GET, "/admin/learning/heatmap?axis=difficulty", &admin_token, Value::Null).await;
    assert_eq!(before["total_graded"], 0);
    titian_backend_rust::services::metrics_rollup::rollup_day(&pool, titian_backend_rust::services::metrics_rollup::wib_today()).await.unwrap();
    let (_, platform) = send(app.clone(), Method::GET, "/admin/learning/heatmap?axis=difficulty", &admin_token, Value::Null).await;
    assert_eq!(platform["total_graded"], 6, "{platform:?}");
    let mudah = platform["rows"][0]["cells"].as_array().unwrap().iter().find(|c| c["key"] == "mudah").unwrap();
    assert_eq!((mudah["graded"].as_i64(), mudah["correct"].as_i64()), (Some(6), Some(5)));
    let (status, _) = send(app.clone(), Method::GET, "/admin/learning/heatmap", &student_token, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The class teacher sees the class; another teacher does not.
    let org_id: Uuid = sqlx::query_scalar!(r#"select organization_id from user_organization_roles where user_id = $1"#, teacher_uid).fetch_one(&pool).await.unwrap();
    let class_id: Uuid = sqlx::query_scalar!(r#"insert into classes (organization_id, teacher_id, name) values ($1, $2, 'Kelas 4A') returning id"#, org_id, teacher_uid).fetch_one(&pool).await.unwrap();
    sqlx::query!(r#"insert into class_members (class_id, student_id) values ($1, $2)"#, class_id, student_uid).execute(&pool).await.unwrap();
    let (status, class) = send(app.clone(), Method::GET, &format!("/classes/{class_id}/learning-heatmap"), &teacher_token, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{class:?}");
    assert_eq!(class["total_graded"], 6);
    let (status, _) = send(app.clone(), Method::GET, &format!("/classes/{class_id}/learning-heatmap"), &other_teacher_token, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(app.clone(), Method::GET, &format!("/classes/{class_id}/learning-heatmap?student_id={outsider_uid}"), &teacher_token, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a teacher can't look up a student outside their class");
}
