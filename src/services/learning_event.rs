// P39-003 (ADR-0013 L1) — the single writer for `learning_events`,
// replacing the raw `insert into learning_events (...)` that used to be
// scattered inline (assessment.rs had the only one; P39-004/005/006 add
// several more callers). Two things this buys that inline inserts
// can't:
//
//   1. A REGISTRY (`EVENT_TYPES`) is the one place every event type's
//      shape and allowed source (server-authoritative vs. the future
//      client telemetry batch, `POST /events`) is declared. An unknown
//      `event_type`, or a known one called from a channel it doesn't
//      permit, is rejected outright rather than silently stored — the
//      same "don't let garbage into the table quietly" contract
//      quiz_config_schema.rs already enforces for quiz_config.
//   2. Idempotent writes for client-sourced events via
//      `client_event_id`, so a retried `POST /events` batch (the network
//      dropped the response, not the request) can't double-count.
//
// Reader compatibility (the reason every new column below is additive,
// never a rename): `mastery.rs::find_events_for_concept` and
// `frss.rs::find_last_answered_question_type` both read ONLY
// `event_type`, `entity_type`, `entity_id`, `payload->>'correct'`,
// `payload->>'difficulty'`, `user_id`, `created_at` off `question_answered`
// rows. `record()` still writes every one of those with the exact same
// meaning `assessment.rs`'s old inline insert did — migrating that call
// site to go through here changes nothing those readers can observe.

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

/// Which entry point is trying to write an event — checked against each
/// registry entry's `allowed_channels` before anything is stored. The
/// server channel is every internal service call (`assessment::submit_attempt`,
/// and P39-004's quiz/module/tryout writers); the client channel is
/// P39-005's `POST /events` batch endpoint, which is rate-limited and
/// far less trusted — an event type not explicitly opted into `Client`
/// can never be forged through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventChannel {
    Server,
    Client,
}

pub struct EventTypeInfo {
    pub id: &'static str,
    pub allowed_channels: &'static [EventChannel],
    /// Checks the payload has what this event type's readers need.
    /// Returns a human-readable reason on failure. Deliberately narrow
    /// (only what's actually READ today, per the file header) — an
    /// overzealous schema here would reject a payload some future
    /// reader hasn't been written yet to need.
    pub validate: fn(&serde_json::Value) -> Result<(), String>,
    /// P39-007 — `None` means this event is always recorded regardless
    /// of consent (ADR-0013 §1.4's "event server yang wajib": grading,
    /// progress, purchases — every P39-004 event type). `Some(kind)`
    /// gates it behind `user_data_consent::has_consent(user_id, kind)`
    /// — the client telemetry P39-005 introduces, and P39-006's
    /// `live_chat_question`.
    pub requires_consent: Option<&'static str>,
}

fn validate_question_answered(payload: &serde_json::Value) -> Result<(), String> {
    let obj = payload.as_object().ok_or("payload must be an object")?;
    if !obj.contains_key("correct") {
        return Err(r#"payload must have a "correct" key (bool or null)"#.to_string());
    }
    // Number (the old questions-bank's 0.0-1.0 difficulty) OR string
    // (P39-001's taxonomy — "mudah"/"sedang"/"sulit") OR null — the two
    // producers of this event (the old bank, and P39-004's quiz_config
    // path) genuinely have different difficulty representations, and
    // neither is wrong; a validator that only accepted one would reject
    // the other producer's perfectly good events.
    match obj.get("difficulty") {
        Some(v) if v.is_number() || v.is_string() || v.is_null() => {}
        _ => return Err(r#"payload must have a "difficulty" key (number, string, or null)"#.to_string()),
    }
    Ok(())
}

fn validate_quiz_attempt_submitted(payload: &serde_json::Value) -> Result<(), String> {
    let obj = payload.as_object().ok_or("payload must be an object")?;
    if !obj.contains_key("score") {
        return Err(r#"payload must have a "score" key (number or null)"#.to_string());
    }
    match obj.get("pending_review") {
        Some(serde_json::Value::Bool(_)) => Ok(()),
        _ => Err(r#"payload must have a "pending_review" key (bool)"#.to_string()),
    }
}

fn validate_module_item_completed(payload: &serde_json::Value) -> Result<(), String> {
    let obj = payload.as_object().ok_or("payload must be an object")?;
    match obj.get("score") {
        Some(v) if v.is_number() || v.is_null() => Ok(()),
        _ => Err(r#"payload must have a "score" key (number or null)"#.to_string()),
    }
}

fn validate_question_graded(payload: &serde_json::Value) -> Result<(), String> {
    let obj = payload.as_object().ok_or("payload must be an object")?;
    match obj.get("score") {
        Some(v) if v.is_number() => Ok(()),
        _ => Err(r#"payload must have a "score" key (number, 0-100)"#.to_string()),
    }
}

fn validate_exam_session_lifecycle(payload: &serde_json::Value) -> Result<(), String> {
    if !payload.is_object() {
        return Err("payload must be an object".to_string());
    }
    Ok(())
}

// --- P39-005 — client telemetry (`POST /events`, EventChannel::Client) ---
//
// Every one of these is gated `requires_consent: Some("learning_analytics")`
// and NOTHING reads them yet — no mastery/FRSS-style consumer exists for
// this data today. Validators stay deliberately loose (structural sanity
// only) rather than guessing a shape a future reader might not actually
// need; tightening a validator is a safe additive change, loosening one
// after callers depend on the strict version is not.
fn validate_object_payload(payload: &serde_json::Value) -> Result<(), String> {
    if !payload.is_object() {
        return Err("payload must be an object".to_string());
    }
    Ok(())
}

fn validate_section_read(payload: &serde_json::Value) -> Result<(), String> {
    let obj = payload.as_object().ok_or("payload must be an object")?;
    match obj.get("active_seconds") {
        Some(v) if v.is_number() => Ok(()),
        _ => Err(r#"payload must have an "active_seconds" key (number) — time the tab was actually visible, not wall-clock time"#.to_string()),
    }
}

fn validate_section_scroll_depth(payload: &serde_json::Value) -> Result<(), String> {
    let obj = payload.as_object().ok_or("payload must be an object")?;
    match obj.get("depth_percent").and_then(|v| v.as_f64()) {
        Some(v) if (0.0..=100.0).contains(&v) => Ok(()),
        _ => Err(r#"payload must have a "depth_percent" key (number, 0-100)"#.to_string()),
    }
}

// --- P39-006 — Live AI Chat, one event per student turn ---
//
// Server channel (the backend itself writes this, right where the turn
// is handled — never trusted from `POST /events`), gated behind
// `ai_chat_storage` consent. Only the STUDENT's question is stored, per
// the ticket's own privacy line ("tidak menyimpan jawaban AI") — the
// tutor's reply is regenerable from the question + lesson_plan and
// isn't the part UU No. 27/2022 consent is really about here.
fn validate_live_chat_question(payload: &serde_json::Value) -> Result<(), String> {
    let obj = payload.as_object().ok_or("payload must be an object")?;
    match obj.get("question").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => Ok(()),
        _ => Err(r#"payload must have a non-empty "question" key (string)"#.to_string()),
    }
}

/// The registry — every `event_type` this codebase may write, and the
/// one place its shape and allowed channel are declared. `question_answered`
/// is the event type already in production before this ticket (ported
/// from `assessment.rs`'s inline insert); the other five are new in
/// P39-004. Widening this list never needs a migration — the shape
/// lives in code, not a DB enum.
pub const EVENT_TYPES: &[EventTypeInfo] = &[
    EventTypeInfo { id: "question_answered", allowed_channels: &[EventChannel::Server], validate: validate_question_answered, requires_consent: None },
    EventTypeInfo { id: "quiz_attempt_submitted", allowed_channels: &[EventChannel::Server], validate: validate_quiz_attempt_submitted, requires_consent: None },
    EventTypeInfo { id: "module_item_completed", allowed_channels: &[EventChannel::Server], validate: validate_module_item_completed, requires_consent: None },
    EventTypeInfo { id: "question_graded", allowed_channels: &[EventChannel::Server], validate: validate_question_graded, requires_consent: None },
    EventTypeInfo { id: "exam_session_started", allowed_channels: &[EventChannel::Server], validate: validate_exam_session_lifecycle, requires_consent: None },
    EventTypeInfo { id: "exam_session_finished", allowed_channels: &[EventChannel::Server], validate: validate_exam_session_lifecycle, requires_consent: None },
    // P39-005 — client telemetry. Every one gated behind `learning_analytics`
    // consent; the underlying reading/quiz-taking experience works
    // identically whether or not these are ever recorded (ADR-0013 §1.4).
    EventTypeInfo { id: "section_viewed", allowed_channels: &[EventChannel::Client], validate: validate_object_payload, requires_consent: Some("learning_analytics") },
    EventTypeInfo { id: "section_read", allowed_channels: &[EventChannel::Client], validate: validate_section_read, requires_consent: Some("learning_analytics") },
    EventTypeInfo { id: "section_scroll_depth", allowed_channels: &[EventChannel::Client], validate: validate_section_scroll_depth, requires_consent: Some("learning_analytics") },
    EventTypeInfo { id: "question_viewed", allowed_channels: &[EventChannel::Client], validate: validate_object_payload, requires_consent: Some("learning_analytics") },
    EventTypeInfo { id: "answer_changed", allowed_channels: &[EventChannel::Client], validate: validate_object_payload, requires_consent: Some("learning_analytics") },
    EventTypeInfo { id: "hint_opened", allowed_channels: &[EventChannel::Client], validate: validate_object_payload, requires_consent: Some("learning_analytics") },
    // P39-006 — Live AI Chat. Server-only: only `live_chat.rs` itself
    // ever writes this, right where the turn is authenticated and
    // rate-limited by the AI gateway already, unlike the client
    // telemetry types above.
    EventTypeInfo { id: "live_chat_question", allowed_channels: &[EventChannel::Server], validate: validate_live_chat_question, requires_consent: Some("ai_chat_storage") },
];

/// `pub(crate)` — `client_events.rs` (P39-005) looks a client-supplied
/// event type up here first, so it can pass the registry's OWN
/// `&'static str` id into `NewLearningEvent` instead of needing that
/// struct to accept an owned `String` just for one caller.
pub(crate) fn find_event_type(id: &str) -> Option<&'static EventTypeInfo> {
    EVENT_TYPES.iter().find(|e| e.id == id)
}

/// Everything `record` needs, gathered up front so the function itself
/// stays a straight validate-then-insert with no field-by-field
/// argument list to keep in sync across call sites.
pub struct NewLearningEvent {
    pub event_type: &'static str,
    pub entity_type: &'static str,
    pub entity_id: Uuid,
    pub payload: serde_json::Value,
    /// ADR-0013 §1.5 — which part of the app this happened in.
    pub source: &'static str,
    pub session_id: Option<Uuid>,
    pub org_id: Option<Uuid>,
    pub module_item_id: Option<Uuid>,
    /// A question's `uid` (P39-001) or a Modul Belajar section id.
    pub content_uid: Option<String>,
    pub content_version: Option<i32>,
    /// `None` defaults to "now" at insert time — the right default for
    /// a server-authoritative event, which IS the thing happening.
    pub occurred_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Set only for client-sourced events that need retry-safe
    /// de-duplication. Always `None` for server-authoritative writes —
    /// those already happen inside whatever transaction is doing the
    /// real work (a submit, a purchase), which is its own dedup.
    pub client_event_id: Option<String>,
}

impl NewLearningEvent {
    /// The common case — a server-authoritative event with nothing but
    /// the fields every event has. `.with_*` builders below fill in the
    /// rest only when a caller actually has them.
    pub fn server(event_type: &'static str, entity_type: &'static str, entity_id: Uuid, payload: serde_json::Value, source: &'static str) -> Self {
        Self {
            event_type,
            entity_type,
            entity_id,
            payload,
            source,
            session_id: None,
            org_id: None,
            module_item_id: None,
            content_uid: None,
            content_version: None,
            occurred_at: None,
            client_event_id: None,
        }
    }
}

/// Validates `event` against the registry for the given `channel`, then
/// inserts it. Returns the new row's id, or `None` when P39-007
/// consent gates this event type and `user_id` hasn't granted it —
/// ADR-0013 §1.4: the underlying FEATURE still works (a quiz still
/// grades, a chat still answers), it just isn't recorded. That is a
/// deliberate, silent no-op, not an error — a caller should never need
/// to branch on it, only optionally log the id.
///
/// A de-duplicated repeat of a `client_event_id` already seen returns
/// the id of the ORIGINAL insert instead of writing a second row.
///
/// Takes `user_id` directly rather than an `&AuthContext` — the event's
/// subject is whoever the behaviour belongs to, which is not always the
/// caller: a teacher grading a student's manual-review answer calls
/// this on the STUDENT's behalf (`grade_manual_group`), and a future
/// Phase 42 content agent recording something has no interactive
/// session/AuthContext at all.
pub async fn record(pool: &PgPool, user_id: Uuid, channel: EventChannel, event: NewLearningEvent) -> Result<Option<Uuid>, AppError> {
    let info = find_event_type(event.event_type).ok_or_else(|| AppError::UnprocessableEntity("unknown_event_type", format!(r#"event_type "{}" is not registered"#, event.event_type)))?;
    if !info.allowed_channels.contains(&channel) {
        return Err(AppError::UnprocessableEntity("event_type_not_allowed_on_channel", format!(r#"event_type "{}" may not be written from this channel"#, event.event_type)));
    }
    (info.validate)(&event.payload).map_err(|reason| AppError::UnprocessableEntity("invalid_event_payload", format!(r#"event_type "{}": {reason}"#, event.event_type)))?;

    if let Some(kind) = info.requires_consent {
        if !crate::services::user_data_consent::has_consent(pool, user_id, kind).await? {
            return Ok(None);
        }
    }

    let event_id = Uuid::new_v4();
    let mut tx = pool.begin().await?;

    if let Some(client_event_id) = &event.client_event_id {
        // The idempotency ledger is a SEPARATE, non-partitioned table
        // specifically so this insert can enforce global uniqueness on
        // (user_id, client_event_id) — see the migration's own note on
        // why `learning_events` itself can't (a partitioned table's
        // constraints must include the partition key).
        let existing: Option<Uuid> = sqlx::query_scalar!(
            r#"insert into learning_event_idempotency (user_id, client_event_id, event_id) values ($1, $2, $3)
               on conflict (user_id, client_event_id) do nothing
               returning event_id"#,
            user_id,
            client_event_id,
            event_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if existing.is_none() {
            // Conflict — this client_event_id was already recorded.
            // Return the ORIGINAL event's id, not a new row.
            let original: Uuid = sqlx::query_scalar!(r#"select event_id from learning_event_idempotency where user_id = $1 and client_event_id = $2"#, user_id, client_event_id)
                .fetch_one(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(Some(original));
        }
    }

    sqlx::query!(
        r#"insert into learning_events
             (id, user_id, event_type, entity_type, entity_id, payload, source, session_id, org_id, module_item_id, content_uid, content_version, occurred_at, client_event_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, coalesce($13, now()), $14)"#,
        event_id,
        user_id,
        event.event_type,
        event.entity_type,
        event.entity_id,
        event.payload,
        event.source,
        event.session_id,
        event.org_id,
        event.module_item_id,
        event.content_uid,
        event.content_version,
        event.occurred_at,
        event.client_event_id,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Some(event_id))
}

fn month_bounds(year: i32, month: u32) -> (chrono::NaiveDate, chrono::NaiveDate) {
    let start = chrono::NaiveDate::from_ymd_opt(year, month, 1).expect("valid year/month");
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let end = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("valid year/month");
    (start, end)
}

/// Ensures the current and next calendar month each have a partition,
/// creating whichever is missing. Idempotent (`IF NOT EXISTS`) — safe to
/// call on every boot. This is a standing safety net, not the real
/// operational story: the migration pre-creates partitions through
/// December 2026, and a proper monthly job belongs in Phase 40 once its
/// job queue exists. Table names are built from a `Datelike`-derived
/// year/month, never from caller input, so there's no injection surface
/// despite the `format!`.
pub async fn ensure_current_partitions(pool: &PgPool) -> Result<(), AppError> {
    use chrono::Datelike;
    let now = chrono::Utc::now();
    let (mut year, mut month) = (now.year(), now.month());
    for _ in 0..2 {
        let (start, end) = month_bounds(year, month);
        let table_name = format!("learning_events_{year:04}_{month:02}");
        let sql = format!(r#"create table if not exists {table_name} partition of learning_events for values from ('{start}') to ('{end}')"#);
        sqlx::query(&sql).execute(pool).await.map_err(|e| AppError::Internal(e.into()))?;
        (year, month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    }
    Ok(())
}

/// The name convention `ensure_current_partitions` writes and this
/// reads back: `learning_events_YYYY_MM`. Anything else attached under
/// `learning_events` (only `learning_events_default` today) is left
/// alone — it wouldn't parse and isn't one of ours to judge the age of.
fn parse_partition_month(table_name: &str) -> Option<(i32, u32)> {
    let rest = table_name.strip_prefix("learning_events_")?;
    let (y, m) = rest.split_once('_')?;
    Some((y.parse().ok()?, m.parse().ok()?))
}

/// P39-007 — drops every monthly partition whose content is entirely
/// older than 24 months (ADR-0013's retention window for raw events).
/// Dropping the whole partition table is instant regardless of row
/// count — the entire reason P39-003 partitioned by month instead of
/// leaving this as a row-by-row DELETE that would have to scan however
/// many million rows have piled up by then.
///
/// Not on a schedule yet — called from the same boot-time safety net as
/// `ensure_current_partitions` until Phase 40's job queue can run it
/// properly on a monthly cadence instead of merely "whenever the
/// process happens to restart".
pub async fn enforce_retention(pool: &PgPool) -> Result<Vec<String>, AppError> {
    let cutoff = chrono::Utc::now().date_naive() - chrono::Months::new(24);

    let partitions: Vec<String> = sqlx::query_scalar(
        r#"select c.relname from pg_inherits i
             join pg_class c on c.oid = i.inhrelid
             join pg_class p on p.oid = i.inhparent
           where p.relname = 'learning_events'"#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(e.into()))?;

    let mut dropped = Vec::new();
    for name in partitions {
        let Some((year, month)) = parse_partition_month(&name) else { continue };
        let (_, end) = month_bounds(year, month);
        // The partition's LAST day must also be before the cutoff — a
        // partition still receiving current data is never dropped even
        // if its start month technically qualifies.
        if end <= cutoff {
            let sql = format!("drop table {name}");
            sqlx::query(&sql).execute(pool).await.map_err(|e| AppError::Internal(e.into()))?;
            dropped.push(name);
        }
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn question_answered_requires_correct_and_difficulty_keys() {
        let f = find_event_type("question_answered").unwrap().validate;
        assert!(f(&json!({"correct": true, "difficulty": 0.5})).is_ok(), "the old bank's numeric difficulty");
        assert!(f(&json!({"correct": true, "difficulty": "sedang"})).is_ok(), "P39-001's taxonomy difficulty");
        assert!(f(&json!({"correct": null, "difficulty": null})).is_ok(), "both are legitimately nullable (an ungraded/essay question)");
        assert!(f(&json!({"difficulty": 0.5})).is_err(), "missing correct");
        assert!(f(&json!({"correct": true})).is_err(), "missing difficulty");
        assert!(f(&json!([1, 2])).is_err(), "not an object at all");
    }

    #[test]
    fn quiz_attempt_submitted_requires_score_and_pending_review() {
        let f = find_event_type("quiz_attempt_submitted").unwrap().validate;
        assert!(f(&json!({"score": 87.5, "pending_review": false})).is_ok());
        assert!(f(&json!({"score": null, "pending_review": true})).is_ok(), "still pending manual grading");
        assert!(f(&json!({"pending_review": false})).is_err(), "missing score");
        assert!(f(&json!({"score": 1.0})).is_err(), "missing pending_review");
        assert!(f(&json!({"score": 1.0, "pending_review": "no"})).is_err(), "pending_review must be a real bool");
    }

    #[test]
    fn module_item_completed_requires_a_nullable_score() {
        let f = find_event_type("module_item_completed").unwrap().validate;
        assert!(f(&json!({"score": 92.0})).is_ok());
        assert!(f(&json!({"score": null})).is_ok(), "an article has no score, just completion");
        assert!(f(&json!({})).is_err());
    }

    #[test]
    fn question_graded_requires_a_numeric_score() {
        let f = find_event_type("question_graded").unwrap().validate;
        assert!(f(&json!({"score": 80.0})).is_ok());
        assert!(f(&json!({"score": null})).is_err(), "a grading event without an actual grade is not a graded event");
        assert!(f(&json!({})).is_err());
    }

    #[test]
    fn exam_session_lifecycle_only_requires_an_object() {
        let f = find_event_type("exam_session_started").unwrap().validate;
        assert!(f(&json!({})).is_ok());
        assert!(f(&json!({"deadline": "2026-09-20T00:00:00Z"})).is_ok());
        assert!(f(&json!([1, 2])).is_err());
        assert_eq!(find_event_type("exam_session_finished").unwrap().validate as usize, f as usize, "both lifecycle events share one validator");
    }

    #[test]
    fn every_new_p39_004_event_type_is_server_only() {
        for id in ["quiz_attempt_submitted", "module_item_completed", "question_graded", "exam_session_started", "exam_session_finished"] {
            let info = find_event_type(id).unwrap();
            assert_eq!(info.allowed_channels, &[EventChannel::Server], "{id} must be server-only");
        }
    }

    #[test]
    fn an_unregistered_event_type_is_not_found() {
        assert!(find_event_type("something_made_up").is_none());
    }

    #[test]
    fn question_answered_is_server_only() {
        let info = find_event_type("question_answered").unwrap();
        assert_eq!(info.allowed_channels, &[EventChannel::Server]);
        assert!(!info.allowed_channels.contains(&EventChannel::Client), "must never be forgeable through the future client telemetry endpoint");
    }

    #[test]
    fn month_bounds_handles_the_december_to_january_rollover() {
        let (start, end) = month_bounds(2026, 12);
        assert_eq!(start, chrono::NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
        assert_eq!(end, chrono::NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
    }

    #[test]
    fn month_bounds_covers_exactly_one_month_for_a_mid_year_month() {
        let (start, end) = month_bounds(2026, 6);
        assert_eq!(start, chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap());
        assert_eq!(end, chrono::NaiveDate::from_ymd_opt(2026, 7, 1).unwrap());
    }

    #[test]
    fn parse_partition_month_reads_back_exactly_what_ensure_current_partitions_writes() {
        assert_eq!(parse_partition_month("learning_events_2026_01"), Some((2026, 1)));
        assert_eq!(parse_partition_month("learning_events_2024_12"), Some((2024, 12)));
    }

    #[test]
    fn parse_partition_month_ignores_anything_that_is_not_one_of_ours() {
        assert_eq!(parse_partition_month("learning_events_default"), None, "the catch-all partition has no month to parse");
        assert_eq!(parse_partition_month("something_unrelated"), None);
        assert_eq!(parse_partition_month("learning_events_2026"), None, "missing the month half");
    }
}
