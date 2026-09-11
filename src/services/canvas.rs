use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::conversation::{self, Conversation};

// Port of canvas_service.ts + canvas_repository.ts. P26-001 —
// Collaborative Canvas. 1 conversation (Messaging) = 1 tutor<->student
// pair for 1 session, reusing that pairing/privacy model outright.
//
// NOT to be confused with R8's `class_sessions` (Google Meet
// scheduling) — `canvas_sessions` here is an unrelated collaborative
// writing-document row keyed by conversation_id + item_id.

const VALID_MODES: [&str; 3] = ["learning", "assessment", "exam"];

// Throttle for insert_snapshot — a full revision-history row per
// keystroke would defeat the point of a snapshot. Per-process in-memory
// map, same single-instance assumption the WS broadcast hub makes.
const SNAPSHOT_THROTTLE: std::time::Duration = std::time::Duration::from_secs(10);
static LAST_SNAPSHOT_AT: OnceLock<Mutex<HashMap<Uuid, Instant>>> = OnceLock::new();

fn last_snapshot_at() -> &'static Mutex<HashMap<Uuid, Instant>> {
    LAST_SNAPSHOT_AT.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CanvasSession {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub item_id: Uuid,
    pub mode: String,
    pub status: String,
    pub content: String,
    pub version: i32,
    pub submitted_attempt_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CanvasEvent {
    pub id: Uuid,
    pub session_id: Uuid,
    pub actor_id: Uuid,
    pub r#type: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

async fn insert_session(pool: &PgPool, conversation_id: Uuid, item_id: Uuid) -> Result<CanvasSession, AppError> {
    let row = sqlx::query_as!(
        CanvasSession,
        r#"insert into canvas_sessions (conversation_id, item_id) values ($1, $2)
           returning id, conversation_id, item_id, mode, status, content, version, submitted_attempt_id, created_at, closed_at"#,
        conversation_id,
        item_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn find_session_by_id(pool: &PgPool, id: Uuid) -> Result<Option<CanvasSession>, AppError> {
    let row = sqlx::query_as!(
        CanvasSession,
        r#"select id, conversation_id, item_id, mode, status, content, version, submitted_attempt_id, created_at, closed_at from canvas_sessions where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn list_active_by_conversation(pool: &PgPool, conversation_id: Uuid) -> Result<Vec<CanvasSession>, AppError> {
    let rows = sqlx::query_as!(
        CanvasSession,
        r#"select id, conversation_id, item_id, mode, status, content, version, submitted_attempt_id, created_at, closed_at
           from canvas_sessions where conversation_id = $1 and status = 'active' order by created_at desc"#,
        conversation_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn update_content(pool: &PgPool, id: Uuid, content: &str, version: i32) -> Result<CanvasSession, AppError> {
    let row = sqlx::query_as!(
        CanvasSession,
        r#"update canvas_sessions set content = $2, version = $3 where id = $1
           returning id, conversation_id, item_id, mode, status, content, version, submitted_attempt_id, created_at, closed_at"#,
        id,
        content,
        version,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn update_mode(pool: &PgPool, id: Uuid, mode: &str) -> Result<CanvasSession, AppError> {
    let row = sqlx::query_as!(
        CanvasSession,
        r#"update canvas_sessions set mode = $2 where id = $1
           returning id, conversation_id, item_id, mode, status, content, version, submitted_attempt_id, created_at, closed_at"#,
        id,
        mode,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn insert_event(pool: &PgPool, session_id: Uuid, actor_id: Uuid, r#type: &str, payload: serde_json::Value) -> Result<CanvasEvent, AppError> {
    let row = sqlx::query_as!(
        CanvasEvent,
        r#"insert into canvas_events (session_id, actor_id, type, payload) values ($1, $2, $3, $4)
           returning id, session_id, actor_id, type, payload, created_at"#,
        session_id,
        actor_id,
        r#type,
        payload,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn list_events(pool: &PgPool, session_id: Uuid) -> Result<Vec<CanvasEvent>, AppError> {
    let rows = sqlx::query_as!(
        CanvasEvent,
        r#"select id, session_id, actor_id, type, payload, created_at from canvas_events where session_id = $1 order by created_at asc"#,
        session_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn insert_snapshot(pool: &PgPool, session_id: Uuid, content: &str, version: i32) -> Result<(), AppError> {
    sqlx::query!(r#"insert into canvas_snapshots (session_id, content, version) values ($1, $2, $3)"#, session_id, content, version).execute(pool).await?;
    Ok(())
}

async fn load_conversation(pool: &PgPool, conversation_id: Uuid) -> Result<Conversation, AppError> {
    conversation::find_by_id(pool, conversation_id).await?.ok_or(AppError::NotFound("conversation_not_found"))
}

fn is_participant(conversation: &Conversation, user_id: Uuid) -> bool {
    conversation.student_id == user_id || conversation.tutor_id == user_id
}

// POST /conversations/{id}/canvas-sessions
pub async fn create_session(pool: &PgPool, ctx: &AuthContext, conversation_id: Uuid, item_id: Uuid) -> Result<CanvasSession, AppError> {
    let conversation = load_conversation(pool, conversation_id).await?;
    if !is_participant(&conversation, ctx.user_id) {
        return Err(AppError::Forbidden);
    }

    // Phase 37 — a Canvas session is a live collaborative essay
    // composition, so it needs a `quiz` item carrying at least one
    // `essay`-subtype question group (was: content_type='writing').
    let row = sqlx::query!(r#"select content_type, quiz_config from module_items where id = $1"#, item_id).fetch_optional(pool).await?;
    let Some(row) = row else { return Err(AppError::NotFound("module_item_not_found")) };
    let has_essay_group = row.content_type.as_deref() == Some("quiz")
        && row
            .quiz_config
            .as_ref()
            .and_then(|c| c.get("question_groups"))
            .and_then(|g| g.as_array())
            .is_some_and(|groups| groups.iter().any(|g| g.get("type").and_then(|v| v.as_str()) == Some("essay")));
    if !has_essay_group {
        return Err(AppError::UnprocessableEntity("not_a_writing_item", "canvas sessions require a quiz item with an essay question group".to_string()));
    }

    insert_session(pool, conversation_id, item_id).await
}

// GET /conversations/{id}/canvas-sessions
pub async fn list_active_sessions(pool: &PgPool, ctx: &AuthContext, conversation_id: Uuid) -> Result<Vec<CanvasSession>, AppError> {
    let conversation = load_conversation(pool, conversation_id).await?;
    if !is_participant(&conversation, ctx.user_id) {
        return Err(AppError::Forbidden);
    }
    list_active_by_conversation(pool, conversation_id).await
}

pub struct CanvasSessionDetail {
    pub session: CanvasSession,
    pub events: Vec<CanvasEvent>,
}

async fn load_own_session(pool: &PgPool, ctx: &AuthContext, session_id: Uuid) -> Result<CanvasSession, AppError> {
    let session = find_session_by_id(pool, session_id).await?.ok_or(AppError::NotFound("canvas_session_not_found"))?;
    let conversation = load_conversation(pool, session.conversation_id).await?;
    if !is_participant(&conversation, ctx.user_id) {
        return Err(AppError::Forbidden);
    }
    Ok(session)
}

// GET /canvas-sessions/{id}
pub async fn get_session(pool: &PgPool, ctx: &AuthContext, session_id: Uuid) -> Result<CanvasSessionDetail, AppError> {
    let session = load_own_session(pool, ctx, session_id).await?;
    let events = list_events(pool, session_id).await?;
    Ok(CanvasSessionDetail { session, events })
}

pub struct SessionAuthorization {
    pub session: CanvasSession,
    pub conversation: Conversation,
}

// Shared by both the REST get_session path and the WS connect handshake
// — same participant check either way, one code path.
pub async fn authorize_session(pool: &PgPool, ctx: &AuthContext, session_id: Uuid) -> Result<SessionAuthorization, AppError> {
    let session = find_session_by_id(pool, session_id).await?.ok_or(AppError::NotFound("canvas_session_not_found"))?;
    let conversation = load_conversation(pool, session.conversation_id).await?;
    if !is_participant(&conversation, ctx.user_id) {
        return Err(AppError::Forbidden);
    }
    Ok(SessionAuthorization { session, conversation })
}

// Applies a document_updated WS message. Mode enforcement happens HERE
// (server-side): in 'assessment'/'exam' mode, the TUTOR may never
// mutate the student's document.
pub async fn apply_document_update(pool: &PgPool, session: &CanvasSession, conversation: &Conversation, actor_id: Uuid, content: &str) -> Result<CanvasSession, AppError> {
    let is_tutor = actor_id == conversation.tutor_id;
    if is_tutor && session.mode != "learning" {
        return Err(AppError::Forbidden);
    }

    let next_version = session.version + 1;
    let updated = update_content(pool, session.id, content, next_version).await?;

    let due = {
        let mut last_snapshot_at = last_snapshot_at().lock().unwrap();
        let now = Instant::now();
        let is_due = last_snapshot_at.get(&session.id).map(|last| now.duration_since(*last) >= SNAPSHOT_THROTTLE).unwrap_or(true);
        if is_due {
            last_snapshot_at.insert(session.id, now);
        }
        is_due
    };
    if due {
        insert_snapshot(pool, session.id, content, next_version).await?;
    }

    Ok(updated)
}

// Only the tutor may change mode.
pub async fn apply_mode_change(pool: &PgPool, session: &CanvasSession, conversation: &Conversation, actor_id: Uuid, mode: &str) -> Result<CanvasSession, AppError> {
    if actor_id != conversation.tutor_id {
        return Err(AppError::Forbidden);
    }
    if !VALID_MODES.contains(&mode) {
        return Err(AppError::UnprocessableEntity("invalid_mode", format!("mode must be one of {}", VALID_MODES.join(", "))));
    }
    let updated = update_mode(pool, session.id, mode).await?;
    insert_event(pool, session.id, actor_id, "mode_changed", serde_json::json!({"mode": mode})).await?;
    Ok(updated)
}

pub async fn apply_comment(pool: &PgPool, session: &CanvasSession, actor_id: Uuid, text: &str, anchor_offset: Option<i64>) -> Result<CanvasEvent, AppError> {
    insert_event(pool, session.id, actor_id, "comment_created", serde_json::json!({"text": text, "anchor_offset": anchor_offset})).await
}

pub async fn apply_comment_resolved(pool: &PgPool, session: &CanvasSession, actor_id: Uuid, comment_event_id: &str) -> Result<CanvasEvent, AppError> {
    insert_event(pool, session.id, actor_id, "comment_resolved", serde_json::json!({"comment_event_id": comment_event_id})).await
}

async fn close_session(pool: &PgPool, id: Uuid, submitted_attempt_id: Uuid) -> Result<CanvasSession, AppError> {
    let row = sqlx::query_as!(
        CanvasSession,
        r#"update canvas_sessions set status = 'closed', closed_at = now(), submitted_attempt_id = $2 where id = $1
           returning id, conversation_id, item_id, mode, status, content, version, submitted_attempt_id, created_at, closed_at"#,
        id,
        submitted_attempt_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// POST /canvas-sessions/{id}/submit — reuses
// assessment::create_lesson_attempt + ai_writing_evaluation::submit_writing_attempt
// outright, same pipeline the Bun original composes. Only the STUDENT
// may submit their own canvas document.
pub async fn submit_session(
    pool: &PgPool,
    ctx: &AuthContext,
    ai: &dyn crate::services::ai_provider::AIProvider,
    model: &str,
    session_id: Uuid,
) -> Result<crate::services::ai_writing_evaluation::WritingSubmitResponse, AppError> {
    let session = load_own_session(pool, ctx, session_id).await?;
    let conversation = load_conversation(pool, session.conversation_id).await?;
    if ctx.user_id != conversation.student_id {
        return Err(AppError::Forbidden);
    }
    if session.status == "closed" {
        return Err(AppError::Conflict("canvas_session_already_closed"));
    }

    let attempt = crate::services::assessment::create_lesson_attempt(pool, ctx, session.item_id).await?;
    let result = crate::services::ai_writing_evaluation::submit_writing_attempt(pool, ai, model, ctx, attempt.attempt_id, &session.content).await?;
    close_session(pool, session_id, attempt.attempt_id).await?;
    Ok(result)
}
