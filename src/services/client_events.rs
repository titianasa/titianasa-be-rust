// P39-005 (ADR-0013 L1) — `POST /events`, the batch endpoint
// `lib/telemetry.ts` (titian-web) sends to. Far less trusted than a
// server-authoritative call: rate-limited per user, bounded to a
// sensible batch size, and every event in it must be registered on the
// `EventChannel::Client` allowlist (`learning_event.rs`) — an event type
// not explicitly opted into that list can never be forged through here,
// no matter what a client sends.

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::learning_event::{self, EventChannel, NewLearningEvent};

pub const MAX_BATCH_SIZE: usize = 50;
/// Comfortably above the client's own steady-state cadence (a batch of
/// up to 50 roughly every 10s, `lib/telemetry.ts`'s own flush interval
/// — at most a few hundred events/minute in real use); this is a ceiling
/// against a misbehaving or malicious client, not a throttle on normal use.
const MAX_EVENTS_PER_MINUTE: i64 = 600;
const MAX_OCCURRED_AT_AGE_DAYS: i64 = 7;

/// The same `source` enum `learning_events.source` enforces
/// (migration 0047) — checked here too, so a bad value is a clean 422
/// in this event's own result rather than a whole-batch DB error.
const VALID_SOURCES: &[&str] = &["self_learning", "live_class", "tryout", "assignment", "practice", "speaking_room", "live_ai_chat", "canvas", "review"];

#[derive(Debug, serde::Deserialize)]
pub struct ClientEvent {
    pub event_type: String,
    /// The client KNOWS its own context (reading a Modul Belajar vs.
    /// sitting a tryout) — unlike the server events P39-004 writes,
    /// there's no single right answer to hardcode here.
    pub source: String,
    pub module_item_id: Uuid,
    /// A section id (P39-001's short hex fragment — see
    /// `lesson_plan::new_section_id`) or a question's `uid`, depending
    /// on `event_type`. Always required here: every registered client
    /// event type is scoped to one or the other.
    pub content_uid: String,
    pub content_version: Option<i32>,
    pub session_id: Option<Uuid>,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    /// Required (unlike server events) — de-duplicating a retried batch
    /// is the entire reason this field exists; a client event with no
    /// id would defeat that on every network retry.
    pub client_event_id: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
pub struct ClientEventResult {
    pub client_event_id: String,
    /// "recorded" | "skipped_no_consent" | "rejected" — a client that
    /// sees "skipped_no_consent" knows to stop sending (its own consent
    /// cache is stale) rather than retrying forever.
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct SubmitEventsResponse {
    pub results: Vec<ClientEventResult>,
}

/// Question-scoped event types get `entity_type="question"` and
/// `entity_id` parsed from `content_uid` (must be a real uid — P39-001).
/// Everything else is section-scoped: a Modul Belajar section has no
/// UUID of its own (see `content_uid`'s own doc comment), so it's
/// `entity_type="module_item"` / `entity_id=module_item_id`, with the
/// section identified only by `content_uid`.
fn is_question_scoped(event_type: &str) -> bool {
    matches!(event_type, "question_viewed" | "answer_changed" | "hint_opened")
}

async fn check_rate_limit(redis: &redis::Client, user_id: Uuid, batch_len: usize) -> Result<(), AppError> {
    use redis::AsyncCommands;
    let mut conn = match redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        // Redis being unreachable must never block real telemetry —
        // the rate limit is a defense against abuse, not a critical
        // dependency the whole feature goes down with.
        Err(_) => return Ok(()),
    };
    let minute_bucket = chrono::Utc::now().timestamp() / 60;
    let key = format!("ratelimit:events:{user_id}:{minute_bucket}");
    let count: i64 = match conn.incr(&key, batch_len as i64).await {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };
    if count == batch_len as i64 {
        // First write to this minute's bucket — give it a TTL so it's
        // self-cleaning rather than accumulating one key per user per
        // minute forever.
        let _: Result<(), redis::RedisError> = conn.expire(&key, 90).await;
    }
    if count > MAX_EVENTS_PER_MINUTE {
        return Err(AppError::TooManyRequests("event_rate_limit_exceeded"));
    }
    Ok(())
}

pub async fn submit(pool: &PgPool, redis: &redis::Client, ctx: &AuthContext, events: Vec<ClientEvent>) -> Result<SubmitEventsResponse, AppError> {
    if events.len() > MAX_BATCH_SIZE {
        return Err(AppError::UnprocessableEntity("batch_too_large", format!("at most {MAX_BATCH_SIZE} events per request")));
    }
    if events.is_empty() {
        return Ok(SubmitEventsResponse { results: Vec::new() });
    }

    check_rate_limit(redis, ctx.user_id, events.len()).await?;

    let now = chrono::Utc::now();
    let oldest_allowed = now - chrono::Duration::days(MAX_OCCURRED_AT_AGE_DAYS);

    let mut results = Vec::with_capacity(events.len());
    for event in events {
        if event.occurred_at > now || event.occurred_at < oldest_allowed {
            results.push(ClientEventResult { client_event_id: event.client_event_id, status: "rejected", error: Some("occurred_at is outside the allowed 7-day window".to_string()) });
            continue;
        }

        // Resolved to the matching STATIC entry (not the owned `String`
        // the client sent) — `NewLearningEvent::source` is `&'static
        // str`, the same reason `event_type` is looked up through the
        // registry below instead of used directly.
        let Some(source) = VALID_SOURCES.iter().find(|&&s| s == event.source).copied() else {
            results.push(ClientEventResult { client_event_id: event.client_event_id, status: "rejected", error: Some(format!(r#"source "{}" is not recognised"#, event.source)) });
            continue;
        };

        let Some(info) = learning_event::find_event_type(&event.event_type) else {
            results.push(ClientEventResult { client_event_id: event.client_event_id, status: "rejected", error: Some(format!(r#"event_type "{}" is not registered"#, event.event_type)) });
            continue;
        };
        if !info.allowed_channels.contains(&EventChannel::Client) {
            results.push(ClientEventResult { client_event_id: event.client_event_id, status: "rejected", error: Some(format!(r#"event_type "{}" may not be written from the client channel"#, event.event_type)) });
            continue;
        }

        let (entity_type, entity_id) = if is_question_scoped(&event.event_type) {
            match Uuid::parse_str(&event.content_uid) {
                Ok(uid) => ("question", uid),
                Err(_) => {
                    results.push(ClientEventResult { client_event_id: event.client_event_id, status: "rejected", error: Some("content_uid must be a real question uid for this event_type".to_string()) });
                    continue;
                }
            }
        } else {
            ("module_item", event.module_item_id)
        };

        let mut new_event = NewLearningEvent::server(info.id, entity_type, entity_id, event.payload, source);
        new_event.module_item_id = Some(event.module_item_id);
        new_event.content_uid = Some(event.content_uid.clone());
        new_event.content_version = event.content_version;
        new_event.session_id = event.session_id;
        new_event.occurred_at = Some(event.occurred_at);
        new_event.client_event_id = Some(event.client_event_id.clone());

        match learning_event::record(pool, ctx.user_id, EventChannel::Client, new_event).await {
            Ok(Some(_)) => results.push(ClientEventResult { client_event_id: event.client_event_id, status: "recorded", error: None }),
            Ok(None) => results.push(ClientEventResult { client_event_id: event.client_event_id, status: "skipped_no_consent", error: None }),
            Err(e) => results.push(ClientEventResult { client_event_id: event.client_event_id, status: "rejected", error: Some(e.to_string()) }),
        }
    }

    Ok(SubmitEventsResponse { results })
}
