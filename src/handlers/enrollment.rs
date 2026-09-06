use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::enrollment::{self, EnrollmentResponse, MyEnrollmentListResponse, StudentListResponse};
use crate::state::AppState;

// POST /cohorts/{id}/enrollments
pub async fn post_enrollment(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(cohort_id): Path<Uuid>,
) -> Result<(StatusCode, Json<EnrollmentResponse>), AppError> {
    let result = enrollment::enroll_self(&state.db, &ctx, cohort_id).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /cohorts/{id}/students
pub async fn get_students(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(cohort_id): Path<Uuid>,
) -> Result<Json<StudentListResponse>, AppError> {
    Ok(Json(enrollment::list_cohort_students(&state.db, &ctx, cohort_id).await?))
}

// POST /cohorts/{id}/enrollments/{enrollment_id}/complete
pub async fn post_complete(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((cohort_id, enrollment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<EnrollmentResponse>, AppError> {
    Ok(Json(enrollment::complete_enrollment(&state.db, &ctx, cohort_id, enrollment_id).await?))
}

// GET /me/enrollments
pub async fn get_my_enrollments(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<MyEnrollmentListResponse>, AppError> {
    Ok(Json(enrollment::list_my_enrollments(&state.db, &ctx).await?))
}

#[derive(serde::Serialize)]
pub struct CancelResponse {
    pub enrollment_id: Uuid,
    pub status: String,
    pub refund_amount_idr: i64,
}

impl From<enrollment::CancelResult> for CancelResponse {
    fn from(r: enrollment::CancelResult) -> Self {
        Self { enrollment_id: r.enrollment.id, status: r.enrollment.status, refund_amount_idr: r.refund_amount_idr }
    }
}

// POST /enrollments/{id}/cancel
pub async fn post_cancel(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(enrollment_id): Path<Uuid>,
) -> Result<Json<CancelResponse>, AppError> {
    let result = enrollment::cancel_enrollment(&state.db, &ctx, enrollment_id).await?;
    Ok(Json(result.into()))
}

// POST /cohorts/{id}/enrollments/{enrollment_id}/tutor-cancel
pub async fn post_tutor_cancel(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((cohort_id, enrollment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CancelResponse>, AppError> {
    let result = enrollment::tutor_cancel_enrollment(&state.db, &ctx, cohort_id, enrollment_id).await?;
    Ok(Json(result.into()))
}
