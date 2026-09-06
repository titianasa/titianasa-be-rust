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
use crate::models::requests::messaging::CreateCanvasSessionRequest;
use crate::services::canvas::{self, CanvasEvent, CanvasSession};
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct CanvasSessionResponse {
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

impl From<CanvasSession> for CanvasSessionResponse {
    fn from(s: CanvasSession) -> Self {
        Self {
            id: s.id,
            conversation_id: s.conversation_id,
            item_id: s.item_id,
            mode: s.mode,
            status: s.status,
            content: s.content,
            version: s.version,
            submitted_attempt_id: s.submitted_attempt_id,
            created_at: s.created_at,
            closed_at: s.closed_at,
        }
    }
}

// POST /conversations/{id}/canvas-sessions
pub async fn post_canvas_session(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(conversation_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateCanvasSessionRequest>,
) -> Result<(StatusCode, Json<CanvasSessionResponse>), AppError> {
    let session = canvas::create_session(&state.db, &ctx, conversation_id, body.item_id).await?;
    Ok((StatusCode::CREATED, Json(session.into())))
}

#[derive(Debug, serde::Serialize)]
pub struct CanvasSessionListResponse {
    pub items: Vec<CanvasSessionResponse>,
}

// GET /conversations/{id}/canvas-sessions
pub async fn get_canvas_sessions(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(conversation_id): Path<Uuid>) -> Result<Json<CanvasSessionListResponse>, AppError> {
    let sessions = canvas::list_active_sessions(&state.db, &ctx, conversation_id).await?;
    Ok(Json(CanvasSessionListResponse { items: sessions.into_iter().map(Into::into).collect() }))
}

#[derive(Debug, serde::Serialize)]
pub struct CanvasEventResponse {
    pub id: Uuid,
    pub actor_id: Uuid,
    pub r#type: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl From<CanvasEvent> for CanvasEventResponse {
    fn from(e: CanvasEvent) -> Self {
        Self { id: e.id, actor_id: e.actor_id, r#type: e.r#type, payload: e.payload, created_at: e.created_at }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct CanvasSessionDetailResponse {
    pub session: CanvasSessionResponse,
    pub events: Vec<CanvasEventResponse>,
}

// GET /canvas-sessions/{id}
pub async fn get_canvas_session(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<CanvasSessionDetailResponse>, AppError> {
    let detail = canvas::get_session(&state.db, &ctx, id).await?;
    Ok(Json(CanvasSessionDetailResponse { session: detail.session.into(), events: detail.events.into_iter().map(Into::into).collect() }))
}

// POST /canvas-sessions/{id}/submit
pub async fn post_submit(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::services::ai_writing_evaluation::WritingSubmitResponse>, AppError> {
    let result = canvas::submit_session(&state.db, &ctx, state.ai_provider.as_ref(), &state.config.ai_writing_evaluation_model, id).await?;
    Ok(Json(result))
}
