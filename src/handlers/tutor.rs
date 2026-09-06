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
use crate::models::requests::tutor::{AssignTutorRequest, UpdateTutorProfileRequest};
use crate::models::responses::tutor::{TutorListResponse, TutorProfileResponse};
use crate::services::tutor;
use crate::state::AppState;

// POST /organizations/{id}/tutors
pub async fn post_tutor(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(organization_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<AssignTutorRequest>,
) -> Result<(StatusCode, Json<TutorProfileResponse>), AppError> {
    let profile =
        tutor::assign_tutor(&state.db, &ctx, organization_id, body.user_id, body.bio, body.specializations).await?;
    Ok((StatusCode::CREATED, Json(profile)))
}

// GET /organizations/{id}/tutors
pub async fn get_tutors(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<TutorListResponse>, AppError> {
    let items = tutor::list_tutors(&state.db, organization_id).await?;
    Ok(Json(TutorListResponse { items }))
}

// PATCH /tutors/me
pub async fn patch_own_profile(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<UpdateTutorProfileRequest>,
) -> Result<Json<TutorProfileResponse>, AppError> {
    let profile = tutor::update_own_profile(&state.db, &ctx, body.bio, body.specializations).await?;
    Ok(Json(profile))
}
