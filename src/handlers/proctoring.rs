use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::proctoring::{CreateEventRequest, ReviewRequest, StartSessionRequest};
use crate::services::proctoring::{self, EventResponse, ReviewQueueRow, SessionDetailResponse, SessionResponse};
use crate::state::AppState;

// POST /exam-sessions/{id}/proctoring-session
pub async fn post_session(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(exam_session_id): Path<Uuid>, ValidatedJson(body): ValidatedJson<StartSessionRequest>) -> Result<(StatusCode, Json<SessionResponse>), AppError> {
    let result = proctoring::start_proctoring_session(&state.db, &ctx, exam_session_id, body.policy_id, body.consent, body.device_info).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /proctoring-sessions/{id}/events
pub async fn post_event(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(proctoring_session_id): Path<Uuid>, ValidatedJson(body): ValidatedJson<CreateEventRequest>) -> Result<(StatusCode, Json<EventResponse>), AppError> {
    let result = proctoring::record_event(&state.db, &ctx, proctoring_session_id, &body.r#type, &body.severity, body.metadata, body.evidence_id).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /proctoring-sessions
pub async fn get_sessions(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<ReviewQueueListResponse>, AppError> {
    let items = proctoring::list_sessions_for_review(&state.db, &ctx).await?;
    Ok(Json(ReviewQueueListResponse { items }))
}

#[derive(serde::Serialize)]
pub struct ReviewQueueListResponse {
    pub items: Vec<ReviewQueueRow>,
}

// GET /proctoring-sessions/{id}
pub async fn get_session(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<SessionDetailResponse>, AppError> {
    Ok(Json(proctoring::get_session(&state.db, &ctx, id).await?))
}

// POST /proctoring-sessions/{id}/review
pub async fn post_review(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<ReviewRequest>) -> Result<Json<SessionResponse>, AppError> {
    let result = proctoring::submit_review(&state.db, &ctx, id, &body.decision, body.notes.as_deref()).await?;
    Ok(Json(result))
}
