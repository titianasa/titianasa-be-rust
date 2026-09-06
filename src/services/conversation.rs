use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::cohort::can_manage_cohorts;

// Port of messaging_service.ts + conversation_repository.ts +
// message_repository.ts. P22-001 — 1 conversation per (cohort, student),
// reusing the existing enrollment/cohort relationship rather than a
// free DM model.

#[derive(Debug, Clone, serde::Serialize)]
pub struct Conversation {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub student_id: Uuid,
    pub tutor_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConversationWithNames {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub student_id: Uuid,
    pub tutor_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub student_name: String,
    pub tutor_name: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MessageResponse {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub sender_id: Uuid,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Conversation>, AppError> {
    let row = sqlx::query_as!(Conversation, r#"select id, cohort_id, student_id, tutor_id, created_at from conversations where id = $1"#, id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

async fn find_by_id_with_names(pool: &PgPool, id: Uuid) -> Result<Option<ConversationWithNames>, AppError> {
    let row = sqlx::query_as!(
        ConversationWithNames,
        r#"select c.id, c.cohort_id, c.student_id, c.tutor_id, c.created_at,
                  su.name as student_name, tu.name as tutor_name
           from conversations c
           inner join users su on su.id = c.student_id
           inner join users tu on tu.id = c.tutor_id
           where c.id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// Idempotent get-or-create: ON CONFLICT (cohort_id, student_id) DO
// UPDATE (self-update) RETURNING — calling twice returns the same row.
async fn get_or_create(pool: &PgPool, cohort_id: Uuid, student_id: Uuid, tutor_id: Uuid) -> Result<Conversation, AppError> {
    let row = sqlx::query_as!(
        Conversation,
        r#"insert into conversations (cohort_id, student_id, tutor_id) values ($1, $2, $3)
           on conflict (cohort_id, student_id) do update set cohort_id = excluded.cohort_id
           returning id, cohort_id, student_id, tutor_id, created_at"#,
        cohort_id,
        student_id,
        tutor_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// POST /cohorts/{id}/conversations — the caller opens their own thread
// (as the enrolled student) OR, if they manage the cohort, opens one on
// behalf of a specific enrolled student (target_student_id required).
pub async fn open_conversation(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid, target_student_id: Option<Uuid>) -> Result<Conversation, AppError> {
    let (_cohort, product) = crate::services::cohort::find_product_for_cohort_id(pool, cohort_id).await?;
    let manages = can_manage_cohorts(pool, ctx, &product).await?;

    let student_id = if manages {
        target_student_id.ok_or_else(|| AppError::UnprocessableEntity("student_id_required", "student_id is required when opening a conversation as a manager".to_string()))?
    } else {
        ctx.user_id
    };

    let enrollment = sqlx::query_scalar!(r#"select id from enrollments where cohort_id = $1 and student_id = $2"#, cohort_id, student_id).fetch_optional(pool).await?;
    if enrollment.is_none() {
        // Not enrolled: forbidden for the student themself; not_found
        // for a manager naming someone who was never a student here.
        return Err(if manages { AppError::NotFound("student_not_enrolled") } else { AppError::Forbidden });
    }

    get_or_create(pool, cohort_id, student_id, product.tutor_id).await
}

fn is_participant(conversation: &Conversation, user_id: Uuid) -> bool {
    conversation.student_id == user_id || conversation.tutor_id == user_id
}

pub struct ConversationSummary {
    pub conversation: ConversationWithNames,
    pub unread_count: i64,
}

// GET /conversations — the caller's own threads only, either side. No
// org-wide visibility. Names come along so an inbox never shows a raw
// UUID.
pub async fn list_my_conversations(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<ConversationSummary>, AppError> {
    let rows = sqlx::query_as!(
        ConversationWithNames,
        r#"select c.id, c.cohort_id, c.student_id, c.tutor_id, c.created_at,
                  su.name as student_name, tu.name as tutor_name
           from conversations c
           inner join users su on su.id = c.student_id
           inner join users tu on tu.id = c.tutor_id
           where c.tutor_id = $1 or c.student_id = $1"#,
        ctx.user_id,
    )
    .fetch_all(pool)
    .await?;

    let mut summaries = Vec::with_capacity(rows.len());
    for conversation in rows {
        let unread_count = sqlx::query_scalar!(
            r#"select count(*) as "count!" from messages where conversation_id = $1 and sender_id != $2 and read_at is null"#,
            conversation.id,
            ctx.user_id,
        )
        .fetch_one(pool)
        .await?;
        summaries.push(ConversationSummary { conversation, unread_count });
    }
    Ok(summaries)
}

pub(crate) async fn load_own_conversation(pool: &PgPool, ctx: &AuthContext, conversation_id: Uuid) -> Result<Conversation, AppError> {
    let conversation = find_by_id(pool, conversation_id).await?.ok_or(AppError::NotFound("conversation_not_found"))?;
    if !is_participant(&conversation, ctx.user_id) {
        return Err(AppError::Forbidden);
    }
    Ok(conversation)
}

pub struct MessageThread {
    pub conversation: ConversationWithNames,
    pub messages: Vec<MessageResponse>,
}

async fn list_by_conversation(pool: &PgPool, conversation_id: Uuid) -> Result<Vec<MessageResponse>, AppError> {
    let rows = sqlx::query_as!(
        MessageResponse,
        r#"select id, conversation_id, sender_id, body, created_at, read_at from messages where conversation_id = $1 order by created_at asc"#,
        conversation_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// GET /conversations/{id}/messages
pub async fn list_messages(pool: &PgPool, ctx: &AuthContext, conversation_id: Uuid) -> Result<MessageThread, AppError> {
    let conversation = load_own_conversation(pool, ctx, conversation_id).await?;
    let messages = list_by_conversation(pool, conversation_id).await?;
    let with_names = find_by_id_with_names(pool, conversation.id).await?.ok_or_else(|| AppError::Internal(anyhow::anyhow!("conversation vanished between authorization and name lookup")))?;
    Ok(MessageThread { conversation: with_names, messages })
}

// POST /conversations/{id}/messages
pub async fn send_message(pool: &PgPool, ctx: &AuthContext, conversation_id: Uuid, body: &str) -> Result<MessageResponse, AppError> {
    load_own_conversation(pool, ctx, conversation_id).await?;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(AppError::UnprocessableEntity("empty_message", "body must not be empty".to_string()));
    }
    let row = sqlx::query_as!(
        MessageResponse,
        r#"insert into messages (conversation_id, sender_id, body) values ($1, $2, $3)
           returning id, conversation_id, sender_id, body, created_at, read_at"#,
        conversation_id,
        ctx.user_id,
        trimmed,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// POST /conversations/{id}/read — marks every message NOT sent by the
// caller as read, in one shot (not a single message). Idempotent.
pub async fn mark_read(pool: &PgPool, ctx: &AuthContext, conversation_id: Uuid) -> Result<(), AppError> {
    load_own_conversation(pool, ctx, conversation_id).await?;
    sqlx::query!(r#"update messages set read_at = now() where conversation_id = $1 and sender_id != $2 and read_at is null"#, conversation_id, ctx.user_id).execute(pool).await?;
    Ok(())
}
