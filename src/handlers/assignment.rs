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
use crate::models::requests::marketplace::{CreateAssignmentRequest, GradeSubmissionRequest, SubmitAssignmentRequest};
use crate::services::assignment::{self, AssignmentListResponse, AssignmentResponse, SubmissionListResponse, SubmissionResponse};
use crate::services::gradebook::{self, GradebookResponse};
use crate::state::AppState;

// POST /cohorts/{id}/assignments
pub async fn post_assignment(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(cohort_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateAssignmentRequest>,
) -> Result<(StatusCode, Json<AssignmentResponse>), AppError> {
    let result = assignment::create_assignment(&state.db, &ctx, cohort_id, &body.title, body.description.as_deref().unwrap_or(""), body.deadline).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /cohorts/{id}/assignments
pub async fn get_assignments(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(cohort_id): Path<Uuid>,
) -> Result<Json<AssignmentListResponse>, AppError> {
    Ok(Json(assignment::list_assignments(&state.db, &ctx, cohort_id).await?))
}

// POST /assignments/{id}/submissions
pub async fn post_submission(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(assignment_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<SubmitAssignmentRequest>,
) -> Result<(StatusCode, Json<SubmissionResponse>), AppError> {
    let result = assignment::submit_assignment(&state.db, &ctx, assignment_id, &body.content).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /assignments/{id}/submissions
pub async fn get_submissions(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(assignment_id): Path<Uuid>,
) -> Result<Json<SubmissionListResponse>, AppError> {
    Ok(Json(assignment::list_submissions(&state.db, &ctx, assignment_id).await?))
}

// POST /submissions/{id}/grade
pub async fn post_grade(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(submission_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<GradeSubmissionRequest>,
) -> Result<Json<SubmissionResponse>, AppError> {
    let result = assignment::grade_submission(&state.db, &ctx, submission_id, body.score, body.feedback.as_deref()).await?;
    Ok(Json(result))
}

// GET /cohorts/{id}/gradebook
pub async fn get_gradebook(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(cohort_id): Path<Uuid>,
) -> Result<Json<GradebookResponse>, AppError> {
    Ok(Json(gradebook::get_gradebook(&state.db, &ctx, cohort_id).await?))
}
