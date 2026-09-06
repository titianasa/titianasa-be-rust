use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::messaging::{OpenConversationRequest, SendMessageRequest};
use crate::services::conversation::{self, ConversationSummary, ConversationWithNames, MessageResponse};
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct ConversationResponse {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub student_id: Uuid,
    pub tutor_id: Uuid,
    pub created_at: DateTime<Utc>,
}

impl From<conversation::Conversation> for ConversationResponse {
    fn from(c: conversation::Conversation) -> Self {
        Self { id: c.id, cohort_id: c.cohort_id, student_id: c.student_id, tutor_id: c.tutor_id, created_at: c.created_at }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct ConversationWithNamesResponse {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub student_id: Uuid,
    pub tutor_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub student_name: String,
    pub tutor_name: String,
}

impl From<ConversationWithNames> for ConversationWithNamesResponse {
    fn from(c: ConversationWithNames) -> Self {
        Self { id: c.id, cohort_id: c.cohort_id, student_id: c.student_id, tutor_id: c.tutor_id, created_at: c.created_at, student_name: c.student_name, tutor_name: c.tutor_name }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct ConversationSummaryResponse {
    #[serde(flatten)]
    pub conversation: ConversationWithNamesResponse,
    pub unread_count: i64,
}

impl From<ConversationSummary> for ConversationSummaryResponse {
    fn from(s: ConversationSummary) -> Self {
        Self { conversation: s.conversation.into(), unread_count: s.unread_count }
    }
}

// POST /cohorts/{id}/conversations
pub async fn post_open_conversation(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(cohort_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<OpenConversationRequest>,
) -> Result<(StatusCode, Json<ConversationResponse>), AppError> {
    let conversation = conversation::open_conversation(&state.db, &ctx, cohort_id, body.student_id).await?;
    Ok((StatusCode::CREATED, Json(conversation.into())))
}

#[derive(Debug, serde::Serialize)]
pub struct ConversationListResponse {
    pub items: Vec<ConversationSummaryResponse>,
}

// GET /conversations
pub async fn get_conversations(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<ConversationListResponse>, AppError> {
    let summaries = conversation::list_my_conversations(&state.db, &ctx).await?;
    Ok(Json(ConversationListResponse { items: summaries.into_iter().map(Into::into).collect() }))
}

#[derive(Debug, serde::Serialize)]
pub struct MessageThreadResponse {
    pub conversation: ConversationWithNamesResponse,
    pub items: Vec<MessageResponse>,
}

// GET /conversations/{id}/messages
pub async fn get_messages(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<MessageThreadResponse>, AppError> {
    let thread = conversation::list_messages(&state.db, &ctx, id).await?;
    Ok(Json(MessageThreadResponse { conversation: thread.conversation.into(), items: thread.messages }))
}

// POST /conversations/{id}/messages
pub async fn post_message(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<SendMessageRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), AppError> {
    let message = conversation::send_message(&state.db, &ctx, id, &body.body).await?;
    Ok((StatusCode::CREATED, Json(message)))
}

#[derive(Debug, serde::Serialize)]
pub struct OkResponse {
    pub ok: bool,
}

// POST /conversations/{id}/read
pub async fn post_mark_read(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<OkResponse>, AppError> {
    conversation::mark_read(&state.db, &ctx, id).await?;
    Ok(Json(OkResponse { ok: true }))
}
