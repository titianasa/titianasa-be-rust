use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

use titian_backend_rust::{
    routes,
    services::{achievement, daily_mission, streak, xp},
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
    }
}

fn build_app(pool: PgPool) -> axum::Router {
    let state = Arc::new(AppState {
        db: pool,
        redis: redis::Client::open("redis://127.0.0.1:6379").unwrap(),
        config: test_config(),
        google_verifier: titian_backend_rust::services::google_oauth::GoogleTokenVerifier::new(),
        payment_provider: std::sync::Arc::new(titian_backend_rust::services::payment_provider::StubQrisProvider),
        ai_provider: std::sync::Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        text_ai_provider: std::sync::Arc::new(titian_backend_rust::services::ai_provider::FakeAIProvider::success("{}")),
        meeting_provider: std::sync::Arc::new(titian_backend_rust::services::meeting_provider::StubMeetingProvider),
        storage: std::sync::Arc::new(titian_backend_rust::services::storage::InMemoryStorage::new()),
        canvas_hub: std::sync::Arc::new(titian_backend_rust::services::canvas_hub::CanvasHub::new()),
        collab_hub: std::sync::Arc::new(titian_backend_rust::services::collab_hub::CollabHub::new()),
    });
    routes::create_router(state)
}

async fn insert_user(pool: &PgPool, email: &str) -> (Uuid, String) {
    let user_id: Uuid = sqlx::query_scalar!(
        r#"insert into users (google_id, email, name) values ($1, $2, $3) returning id"#,
        format!("google-{email}"),
        email,
        "Test User",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let token = titian_backend_rust::services::token::issue_access_token(JWT_SECRET, &user_id.to_string(), email, 15).unwrap();
    (user_id, token)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
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
async fn award_xp_is_idempotent_by_reference_and_summary_reflects_total(pool: PgPool) {
    let (uid, token) = insert_user(&pool, "xp1@example.com").await;

    let first = xp::award_xp(&pool, uid, 10, "question_answered", Some("ref-1"), Some("grammar")).await.unwrap();
    let second = xp::award_xp(&pool, uid, 10, "question_answered", Some("ref-1"), Some("grammar")).await.unwrap();
    assert!(first);
    assert!(!second);

    let app = build_app(pool);
    let (status, body) = get(app, "/me/xp", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 10);
    assert_eq!(body["recent"][0]["amount"], 10);
}

#[sqlx::test]
async fn streak_advances_consecutively_and_survives_a_freeze(pool: PgPool) {
    let (uid, token) = insert_user(&pool, "streak1@example.com").await;
    let day1 = chrono::Utc::now() - chrono::Duration::days(2);
    let day2 = day1 + chrono::Duration::days(1);
    let day3_gap = day2 + chrono::Duration::days(2); // skip a day — freeze absorbs it

    streak::record_activity(&pool, uid, day1).await.unwrap();
    streak::record_activity(&pool, uid, day2).await.unwrap();
    streak::record_activity(&pool, uid, day3_gap).await.unwrap();

    let app = build_app(pool);
    let (status, body) = get(app, "/me/streak", &token).await;
    assert_eq!(status, StatusCode::OK);
    // day1 -> 1, day2 (consecutive) -> 2, day3_gap (1 day missed, freeze
    // absorbs it) -> stays 2, not reset to 1.
    assert_eq!(body["current_streak"], 2);
    assert_eq!(body["freezes_available"], 0);
}

#[sqlx::test]
async fn achievement_awarded_at_streak_7_and_not_reawarded(pool: PgPool) {
    let (uid, token) = insert_user(&pool, "achieve1@example.com").await;
    let mut day = chrono::Utc::now() - chrono::Duration::days(10);
    for _ in 0..7 {
        streak::record_activity(&pool, uid, day).await.unwrap();
        day += chrono::Duration::days(1);
    }
    achievement::check_and_award(&pool, uid, &achievement::ActivityContext::default()).await.unwrap();
    // Calling again must not duplicate the row (composite PK would
    // reject it, but the earned-codes check should skip the attempt
    // entirely).
    achievement::check_and_award(&pool, uid, &achievement::ActivityContext::default()).await.unwrap();

    let app = build_app(pool);
    let (status, body) = get(app, "/me/achievements", &token).await;
    assert_eq!(status, StatusCode::OK);
    let codes: Vec<&str> = body["items"].as_array().unwrap().iter().map(|a| a["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"streak_7"));
    assert_eq!(codes.iter().filter(|c| **c == "streak_7").count(), 1);
}

#[sqlx::test]
async fn daily_mission_completes_and_awards_xp_once(pool: PgPool) {
    let (uid, token) = insert_user(&pool, "mission1@example.com").await;
    let today = chrono::Utc::now();

    daily_mission::record_progress(&pool, uid, Some("grammar"), today).await.unwrap();
    daily_mission::record_progress(&pool, uid, Some("listening"), today).await.unwrap();
    daily_mission::record_progress(&pool, uid, Some("speaking"), today).await.unwrap();
    for _ in 0..5 {
        daily_mission::record_progress(&pool, uid, Some("vocabulary"), today).await.unwrap();
    }
    // One more vocabulary rep after completion must not double-pay the reward.
    daily_mission::record_progress(&pool, uid, Some("vocabulary"), today).await.unwrap();

    let app = build_app(pool.clone());
    let (status, body) = get(app.clone(), "/me/daily-mission", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["reward_claimed"], true);
    assert_eq!(body["progress"]["vocabulary"], 6);

    let (_, xp_body) = get(app, "/me/xp", &token).await;
    assert_eq!(xp_body["total"], 80);
}

#[sqlx::test]
async fn weekly_leaderboard_ranks_and_computes_percentile(pool: PgPool) {
    let (uid_a, token_a) = insert_user(&pool, "lb-a@example.com").await;
    let (uid_b, _token_b) = insert_user(&pool, "lb-b@example.com").await;
    xp::award_xp(&pool, uid_a, 50, "x", None, None).await.unwrap();
    xp::award_xp(&pool, uid_b, 100, "x", None, None).await.unwrap();

    let app = build_app(pool);
    let (status, body) = get(app, "/leaderboard/weekly", &token_a).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["xp"], 100);
    assert_eq!(body["items"][1]["xp"], 50);
    assert_eq!(body["me"]["rank"], 2);
    assert_eq!(body["me"]["percentile"], 0);
}

#[sqlx::test]
async fn league_score_reflects_streak_and_activity(pool: PgPool) {
    let (uid, token) = insert_user(&pool, "league1@example.com").await;
    streak::record_activity(&pool, uid, chrono::Utc::now()).await.unwrap();
    xp::award_xp(&pool, uid, 5, "x", None, None).await.unwrap();

    let app = build_app(pool);
    let (status, body) = get(app, "/me/league", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["inputs"]["current_streak"], 1);
    assert_eq!(body["inputs"]["weekly_activity_count"], 1);
    assert_eq!(body["tier"], "bronze");
}
