use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, NaiveDate, Utc};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::org_class_session::CreateOrgClassSessionRequest;
use crate::services::org_class_session::{self, OrgClassSessionListResponse, OrgClassSessionResponse};
use crate::state::AppState;

fn parse_datetime(value: &str) -> Result<DateTime<Utc>, AppError> {
    value.parse::<DateTime<Utc>>().map_err(|_| AppError::UnprocessableEntity("invalid_schedule", "must be a valid ISO 8601 datetime".to_string()))
}

fn parse_date(value: &str) -> Result<NaiveDate, AppError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| AppError::UnprocessableEntity("invalid_date", "session_date must be YYYY-MM-DD".to_string()))
}

// POST /classes/{id}/sessions
pub async fn post_session(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(class_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateOrgClassSessionRequest>,
) -> Result<(StatusCode, Json<OrgClassSessionResponse>), AppError> {
    let session_date = parse_date(&body.session_date)?;
    let scheduled_start = parse_datetime(&body.scheduled_start)?;
    let scheduled_end = parse_datetime(&body.scheduled_end)?;
    let session = org_class_session::create_session(&state.db, state.meeting_provider.as_ref(), &ctx, class_id, session_date, scheduled_start, scheduled_end).await?;
    Ok((StatusCode::CREATED, Json(session)))
}

// GET /classes/{id}/sessions
pub async fn get_sessions(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(class_id): Path<Uuid>) -> Result<Json<OrgClassSessionListResponse>, AppError> {
    Ok(Json(org_class_session::list_sessions(&state.db, &ctx, class_id).await?))
}

// GET /org-class-sessions/{id}
pub async fn get_session(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<OrgClassSessionResponse>, AppError> {
    Ok(Json(org_class_session::get_session(&state.db, &ctx, id).await?))
}

// POST /org-class-sessions/{id}/sync-attendance
pub async fn post_sync_attendance(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::services::org_attendance_verification::SyncOrgAttendanceResult>, AppError> {
    let result = crate::services::org_attendance_verification::sync_attendance(&state.db, &state.config, state.meeting_provider.as_ref(), &ctx, id).await?;
    Ok(Json(result))
}
