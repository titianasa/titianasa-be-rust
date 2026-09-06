use axum::{
    extract::{Path, State},
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::class_session::MarkAttendanceRequest;
use crate::services::attendance::{self, AttendanceEntry, AttendanceRecordResponse};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct AttendanceListResponse {
    pub items: Vec<AttendanceRecordResponse>,
}

// POST /cohorts/{id}/sessions/{session_date}/attendance
pub async fn post_attendance(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((cohort_id, session_date)): Path<(Uuid, String)>,
    ValidatedJson(body): ValidatedJson<MarkAttendanceRequest>,
) -> Result<Json<AttendanceListResponse>, AppError> {
    let entries = body.records.into_iter().map(|r| AttendanceEntry { student_id: r.student_id, status: r.status }).collect();
    let items = attendance::mark_attendance(&state.db, &ctx, cohort_id, &session_date, entries).await?;
    Ok(Json(AttendanceListResponse { items }))
}

// GET /cohorts/{id}/attendance
pub async fn get_attendance(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(cohort_id): Path<Uuid>) -> Result<Json<AttendanceListResponse>, AppError> {
    let items = attendance::get_cohort_attendance(&state.db, &ctx, cohort_id).await?;
    Ok(Json(AttendanceListResponse { items }))
}
