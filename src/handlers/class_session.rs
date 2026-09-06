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
use crate::models::requests::class_session::{CreateClassSessionRequest, SimulateParticipantRequest};
use crate::services::attendance_verification::{self, SessionAttendanceRow, SimulateParticipantParams};
use crate::services::class_session::{self, ClassSessionListResponse, ClassSessionResponse};
use crate::services::meeting_provider::RecordingStatus;
use crate::state::AppState;

fn parse_datetime(value: &str) -> Result<DateTime<Utc>, AppError> {
    value.parse::<DateTime<Utc>>().map_err(|_| AppError::UnprocessableEntity("invalid_schedule", "must be a valid ISO 8601 datetime".to_string()))
}

// POST /cohorts/{id}/class-sessions
pub async fn post_class_session(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(cohort_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateClassSessionRequest>,
) -> Result<(StatusCode, Json<ClassSessionResponse>), AppError> {
    let scheduled_start = parse_datetime(&body.scheduled_start)?;
    let scheduled_end = parse_datetime(&body.scheduled_end)?;
    let session = class_session::create_session(&state.db, state.meeting_provider.as_ref(), &ctx, cohort_id, &body.session_date, scheduled_start, scheduled_end).await?;
    Ok((StatusCode::CREATED, Json(session)))
}

// GET /cohorts/{id}/class-sessions
pub async fn get_class_sessions(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(cohort_id): Path<Uuid>) -> Result<Json<ClassSessionListResponse>, AppError> {
    Ok(Json(class_session::list_sessions(&state.db, &ctx, cohort_id).await?))
}

// GET /class-sessions/{id}
pub async fn get_class_session(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ClassSessionResponse>, AppError> {
    Ok(Json(class_session::get_session(&state.db, &ctx, id).await?))
}

// GET /class-sessions/{id}/recording
pub async fn get_recording(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<RecordingStatus>, AppError> {
    Ok(Json(class_session::get_recording_status(&state.db, state.meeting_provider.as_ref(), &ctx, id).await?))
}

#[derive(serde::Serialize)]
pub struct SimulateParticipantResponse {
    pub ok: bool,
}

// POST /class-sessions/{id}/simulate-participant
pub async fn post_simulate_participant(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(class_session_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<SimulateParticipantRequest>,
) -> Result<Json<SimulateParticipantResponse>, AppError> {
    attendance_verification::simulate_participant(
        &state.db,
        &state.config,
        &ctx,
        class_session_id,
        SimulateParticipantParams { role: body.role, user_id: body.user_id, join_offset_minutes: body.join_offset_minutes, duration_minutes: body.duration_minutes },
    )
    .await?;
    Ok(Json(SimulateParticipantResponse { ok: true }))
}

#[derive(serde::Serialize)]
pub struct SyncAttendanceResponse {
    pub session: ClassSessionResponse,
    pub student_records: i64,
}

// POST /class-sessions/{id}/sync-attendance
pub async fn post_sync_attendance(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(class_session_id): Path<Uuid>) -> Result<Json<SyncAttendanceResponse>, AppError> {
    let result = attendance_verification::sync_attendance(&state.db, &state.config, state.meeting_provider.as_ref(), &ctx, class_session_id).await?;
    Ok(Json(SyncAttendanceResponse { session: result.session, student_records: result.student_records }))
}

#[derive(serde::Serialize)]
pub struct SessionAttendanceListResponse {
    pub items: Vec<SessionAttendanceRow>,
}

// GET /class-sessions/{id}/attendance
pub async fn get_session_attendance(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(class_session_id): Path<Uuid>) -> Result<Json<SessionAttendanceListResponse>, AppError> {
    let items = attendance_verification::get_session_attendance_report(&state.db, &ctx, class_session_id).await?;
    Ok(Json(SessionAttendanceListResponse { items }))
}
