// P39-003 (ADR-0013 L1) — proves `assessment::submit_attempt`'s
// migrated write (raw `insert into learning_events` -> `learning_event
// ::record`) is functionally IDENTICAL to what mastery.rs/frss.rs read,
// by exercising the real service function end to end and then running
// mastery's own recompute against the row it wrote.
//
// This calls `assessment::submit_attempt` directly rather than through
// `POST /attempts/{id}/submit`: the HTTP-level fixture every other
// attempt-submission test shares (`attempt_submission_test.rs`) creates
// its module item with `content_type: "learn"`, a value `module_item.rs`
// stopped accepting back in Phase 37 (`CONTENT_TYPES = ["article",
// "quiz"]`) — a PRE-EXISTING, unrelated gap (tracked in STATE.md, not
// part of this ticket) that currently fails all 6 of that file's tests
// before they ever reach a submit. `assessment::submit_attempt` itself
// has no dependency on `module_items` at all (only `assessments`/
// `questions`/`assessment_questions`/`attempts`), so seeding those
// tables directly sidesteps that gap entirely rather than needing to
// fix it to get real coverage of this ticket's own change.

use sqlx::PgPool;
use uuid::Uuid;

use titian_backend_rust::{models::auth::AuthContext, services::assessment, Config};

fn test_config() -> Config {
    Config {
        bind_addr: "0.0.0.0:0".into(),
        database_url: String::new(),
        jwt_access_secret: "test-jwt-secret".into(),
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

/// Seeds one MCQ question, in its own bank/assessment, linked to one
/// concept, and an `in_progress` attempt on it — the minimum
/// `submit_attempt` actually touches.
async fn seed_gradable_attempt(pool: &PgPool, user_id: Uuid) -> (Uuid, Uuid, Uuid) {
    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('SUBJ-LE', 'SUBJ-LE') returning id"#).fetch_one(pool).await.unwrap();
    let concept_id: Uuid = sqlx::query_scalar!(r#"insert into concepts (subject_id, code, name, type) values ($1, 'C-LE', 'Concept LE', 'skill') returning id"#, subject_id)
        .fetch_one(pool)
        .await
        .unwrap();
    let bank_id: Uuid = sqlx::query_scalar!(r#"insert into question_banks (subject_id, name) values ($1, 'Bank LE') returning id"#, subject_id).fetch_one(pool).await.unwrap();
    let question_id: Uuid = sqlx::query_scalar!(
        r#"insert into questions (bank_id, type, difficulty, data, correct_answer, status)
           values ($1, 'multiple_choice', 0.6, $2, $3, 'published') returning id"#,
        bank_id,
        serde_json::json!({"stem": "1 + 1 = ?", "options": ["1", "2"]}),
        serde_json::json!({"index": 1}),
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query!(r#"insert into question_concepts (question_id, concept_id) values ($1, $2)"#, question_id, concept_id).execute(pool).await.unwrap();

    let assessment_id: Uuid = sqlx::query_scalar!(r#"insert into assessments (type, title, config) values ('unit_test', 'Assessment LE', '{}') returning id"#).fetch_one(pool).await.unwrap();
    sqlx::query!(r#"insert into assessment_questions (assessment_id, question_id, order_index) values ($1, $2, 0)"#, assessment_id, question_id).execute(pool).await.unwrap();

    let attempt_id: Uuid = sqlx::query_scalar!(r#"insert into attempts (user_id, assessment_id, status) values ($1, $2, 'in_progress') returning id"#, user_id, assessment_id)
        .fetch_one(pool)
        .await
        .unwrap();

    (attempt_id, question_id, concept_id)
}

#[sqlx::test]
async fn submit_attempt_writes_a_learning_event_readable_by_mastery(pool: PgPool) {
    let config = test_config();
    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-le', 'le@example.com', 'LE User') returning id"#).fetch_one(&pool).await.unwrap();
    let ctx = AuthContext { user_id, organization_id: None, role: Some("student".to_string()) };

    let (attempt_id, question_id, concept_id) = seed_gradable_attempt(&pool, user_id).await;

    let answers = std::collections::HashMap::from([(question_id, serde_json::json!({"index": 1}))]);
    let result = assessment::submit_attempt(&pool, &config, &ctx, attempt_id, &answers).await.unwrap();
    assert_eq!(result.score, 100.0, "the correct answer was submitted");

    // The row the migrated writer produced — same shape as before P39-003.
    let row = sqlx::query!(
        r#"select event_type, entity_type, entity_id, source, schema_version, occurred_at is not null as "has_occurred_at!",
                  (payload->>'correct')::boolean as correct, (payload->>'difficulty')::float8 as difficulty
           from learning_events where user_id = $1 and entity_id = $2"#,
        user_id,
        question_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.event_type, "question_answered");
    assert_eq!(row.entity_type, "question");
    assert_eq!(row.source, "practice");
    assert_eq!(row.schema_version, 1);
    assert!(row.has_occurred_at);
    assert_eq!(row.correct, Some(true));
    assert_eq!(row.difficulty, Some(0.6));

    // The real proof: mastery.rs's OWN reader (unchanged by this ticket)
    // can see the event and compute a score from it.
    titian_backend_rust::services::mastery::recompute_for_concept(&pool, &config, user_id, concept_id).await.unwrap();
    let mastery_score: Option<f64> = sqlx::query_scalar!(r#"select score from masteries where user_id = $1 and concept_id = $2"#, user_id, concept_id).fetch_optional(&pool).await.unwrap();
    assert_eq!(mastery_score, Some(100.0), "one correct answer at full confidence-weight should score 100");
}

#[sqlx::test]
async fn a_repeated_client_event_id_is_deduplicated_not_double_counted(pool: PgPool) {
    use titian_backend_rust::services::learning_event::{record, EventChannel, NewLearningEvent};

    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-dedup', 'dedup@example.com', 'Dedup User') returning id"#)
        .fetch_one(&pool)
        .await
        .unwrap();
    let question_id = Uuid::new_v4();

    let mut event = NewLearningEvent::server("question_answered", "question", question_id, serde_json::json!({"correct": true, "difficulty": 0.4}), "practice");
    event.client_event_id = Some("retry-key-1".to_string());

    let first_id = record(&pool, user_id, EventChannel::Server, event).await.unwrap();

    let mut retry = NewLearningEvent::server("question_answered", "question", question_id, serde_json::json!({"correct": true, "difficulty": 0.4}), "practice");
    retry.client_event_id = Some("retry-key-1".to_string());
    let second_id = record(&pool, user_id, EventChannel::Server, retry).await.unwrap();

    assert_eq!(first_id, second_id, "a retried client_event_id must resolve to the SAME event, not a new one");
    let count: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from learning_events where user_id = $1 and entity_id = $2"#, user_id, question_id).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1, "only one row must actually exist despite two record() calls");
}

#[sqlx::test]
async fn an_unregistered_event_type_is_rejected_not_silently_stored(pool: PgPool) {
    use titian_backend_rust::services::learning_event::{record, EventChannel, NewLearningEvent};

    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-unk', 'unk@example.com', 'Unk User') returning id"#).fetch_one(&pool).await.unwrap();

    let event = NewLearningEvent::server("made_up_event_type", "question", Uuid::new_v4(), serde_json::json!({}), "practice");
    let err = record(&pool, user_id, EventChannel::Server, event).await.unwrap_err();
    assert!(format!("{err:?}").contains("unknown_event_type"), "{err:?}");
}

#[sqlx::test]
async fn exam_session_start_and_finish_each_write_their_own_lifecycle_event(pool: PgPool) {
    use titian_backend_rust::services::exam_session;

    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-exam', 'exam@example.com', 'Exam User') returning id"#).fetch_one(&pool).await.unwrap();
    let ctx = AuthContext { user_id, organization_id: None, role: Some("student".to_string()) };
    let assessment_id: Uuid = sqlx::query_scalar!(r#"insert into assessments (type, title, config) values ('mock_exam', 'Tryout', '{"duration_minutes": 30}') returning id"#).fetch_one(&pool).await.unwrap();

    let started = exam_session::start_exam_session(&pool, &ctx, assessment_id).await.unwrap();

    let start_row = sqlx::query!(r#"select source, session_id from learning_events where event_type = 'exam_session_started' and entity_id = $1"#, started.exam_session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(start_row.source, "tryout", "an exam session IS the tryout context, by definition");
    assert_eq!(start_row.session_id, Some(started.exam_session_id));

    exam_session::record_submission(&pool, assessment_id, user_id, chrono::Utc::now()).await.unwrap();

    let finish_row = sqlx::query!(
        r#"select source, (payload->>'timed_out')::boolean as timed_out from learning_events where event_type = 'exam_session_finished' and entity_id = $1"#,
        started.exam_session_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(finish_row.source, "tryout");
    assert_eq!(finish_row.timed_out, Some(false), "submitted well inside the 30-minute window");
}

#[sqlx::test]
async fn a_server_only_event_type_is_rejected_on_the_client_channel(pool: PgPool) {
    use titian_backend_rust::services::learning_event::{record, EventChannel, NewLearningEvent};

    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-chan', 'chan@example.com', 'Chan User') returning id"#).fetch_one(&pool).await.unwrap();

    let event = NewLearningEvent::server("question_answered", "question", Uuid::new_v4(), serde_json::json!({"correct": true, "difficulty": 0.5}), "practice");
    let err = record(&pool, user_id, EventChannel::Client, event).await.unwrap_err();
    assert!(format!("{err:?}").contains("event_type_not_allowed_on_channel"), "{err:?}");
}

#[sqlx::test]
async fn enforce_retention_drops_only_partitions_older_than_24_months(pool: PgPool) {
    use titian_backend_rust::services::learning_event::enforce_retention;

    // A genuinely ancient partition — guaranteed to be >24 months old
    // no matter when this test runs — alongside one from the migration's
    // own pre-created 2026 range, which must survive untouched.
    sqlx::query("create table learning_events_2018_01 partition of learning_events for values from ('2018-01-01') to ('2018-02-01')")
        .execute(&pool)
        .await
        .unwrap();

    let dropped = enforce_retention(&pool).await.unwrap();
    assert!(dropped.contains(&"learning_events_2018_01".to_string()), "{dropped:?}");
    assert!(!dropped.contains(&"learning_events_2026_01".to_string()), "a current-year partition must never be dropped: {dropped:?}");

    let still_exists: bool = sqlx::query_scalar("select exists(select 1 from pg_tables where tablename = 'learning_events_2018_01')").fetch_one(&pool).await.unwrap();
    assert!(!still_exists, "the dropped partition's table must actually be gone");

    // Idempotent — nothing left to drop on a second call.
    let dropped_again = enforce_retention(&pool).await.unwrap();
    assert!(!dropped_again.contains(&"learning_events_2018_01".to_string()));
}

#[sqlx::test]
async fn set_and_query_consent_round_trips_grant_and_revoke(pool: PgPool) {
    use titian_backend_rust::services::user_data_consent::{has_consent, set_consent};

    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-consent', 'consent@example.com', 'Consent User') returning id"#).fetch_one(&pool).await.unwrap();

    // Never asked -> the safe default.
    assert!(!has_consent(&pool, user_id, "learning_analytics").await.unwrap());

    set_consent(&pool, user_id, "learning_analytics", true, user_id, false, false).await.unwrap();
    assert!(has_consent(&pool, user_id, "learning_analytics").await.unwrap());

    // Revoking must take effect immediately — the DoD's own wording.
    set_consent(&pool, user_id, "learning_analytics", false, user_id, false, false).await.unwrap();
    assert!(!has_consent(&pool, user_id, "learning_analytics").await.unwrap());

    // The two kinds are independent.
    assert!(!has_consent(&pool, user_id, "ai_chat_storage").await.unwrap());
}

// Current uji coba (pilot) behaviour, 2026-09-13: a minor may self-grant
// without a guardian step — `Config::consent_guardian_confirmation_required`
// is `false` by default. The MECHANISM is still fully built (proven by
// the next test); only its enforcement is switched off for the pilot.
#[sqlx::test]
async fn during_the_pilot_a_minor_may_self_grant_consent_without_a_guardian(pool: PgPool) {
    use titian_backend_rust::services::user_data_consent::{has_consent, set_consent};

    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-minor-pilot', 'minor-pilot@example.com', 'Minor Pilot') returning id"#).fetch_one(&pool).await.unwrap();
    sqlx::query!(r#"insert into user_learning_profiles (user_id, jenjang) values ($1, 'SMP-8')"#, user_id).execute(&pool).await.unwrap();

    set_consent(&pool, user_id, "learning_analytics", true, user_id, false, false).await.unwrap();
    assert!(has_consent(&pool, user_id, "learning_analytics").await.unwrap());
}

#[sqlx::test]
async fn a_minor_jenjang_cannot_grant_consent_without_guardian_confirmation_when_enforced(pool: PgPool) {
    use titian_backend_rust::services::user_data_consent::{has_consent, set_consent};

    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-minor', 'minor@example.com', 'Minor User') returning id"#).fetch_one(&pool).await.unwrap();
    sqlx::query!(r#"insert into user_learning_profiles (user_id, jenjang) values ($1, 'SMP-8')"#, user_id).execute(&pool).await.unwrap();

    let err = set_consent(&pool, user_id, "learning_analytics", true, user_id, false, true).await.unwrap_err();
    assert!(format!("{err:?}").contains("guardian_confirmation_required"), "{err:?}");
    assert!(!has_consent(&pool, user_id, "learning_analytics").await.unwrap(), "the rejected grant must not have taken effect");

    // With confirmation, the same grant succeeds.
    set_consent(&pool, user_id, "learning_analytics", true, user_id, true, true).await.unwrap();
    assert!(has_consent(&pool, user_id, "learning_analytics").await.unwrap());

    // Revoking a minor's consent needs no guardian step, even enforced.
    set_consent(&pool, user_id, "learning_analytics", false, user_id, false, true).await.unwrap();
    assert!(!has_consent(&pool, user_id, "learning_analytics").await.unwrap());
}

#[sqlx::test]
async fn an_adult_jenjang_needs_no_guardian_confirmation_even_when_enforced(pool: PgPool) {
    use titian_backend_rust::services::user_data_consent::{has_consent, set_consent};

    let user_id: Uuid = sqlx::query_scalar!(r#"insert into users (google_id, email, name) values ('google-adult', 'adult@example.com', 'Adult User') returning id"#).fetch_one(&pool).await.unwrap();
    sqlx::query!(r#"insert into user_learning_profiles (user_id, jenjang) values ($1, 'SMA-11')"#, user_id).execute(&pool).await.unwrap();

    set_consent(&pool, user_id, "learning_analytics", true, user_id, false, true).await.unwrap();
    assert!(has_consent(&pool, user_id, "learning_analytics").await.unwrap());
}
