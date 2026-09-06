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
use crate::models::requests::marketplace::CreateReviewRequest;
use crate::services::tutor_review::{self, ReputationResponse, ReviewResponse};
use crate::state::AppState;

// POST /tutors/{id}/reviews
pub async fn post_review(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(tutor_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateReviewRequest>,
) -> Result<(StatusCode, Json<ReviewResponse>), AppError> {
    let result = tutor_review::submit_review(&state.db, &ctx, tutor_id, body.enrollment_id, body.rating, body.comment.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /tutors/{id}/reputation — PUBLIC, no auth extractor.
pub async fn get_reputation(State(state): State<Arc<AppState>>, Path(tutor_id): Path<Uuid>) -> Result<Json<ReputationResponse>, AppError> {
    Ok(Json(tutor_review::get_reputation(&state.db, tutor_id).await?))
}
