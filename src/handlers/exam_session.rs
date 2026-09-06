use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::exam_session::{self, ExamSessionResponse, StartExamSessionResponse};
use crate::state::AppState;

// POST /assessments/{id}/exam-sessions
pub async fn post_exam_session(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(assessment_id): Path<Uuid>,
) -> Result<(StatusCode, Json<StartExamSessionResponse>), AppError> {
    let result = exam_session::start_exam_session(&state.db, &ctx, assessment_id).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /exam-sessions/{id}
pub async fn get_exam_session(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ExamSessionResponse>, AppError> {
    Ok(Json(exam_session::get_exam_session(&state.db, &ctx, id).await?))
}
