use axum::{extract::State, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::services::learning_profile;
use crate::state::AppState;

// GET /me/learning-profile
pub async fn get_my_profile(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Option<learning_profile::LearningProfile>>, AppError> {
    Ok(Json(learning_profile::get(&state.db, &ctx).await?))
}

// PUT /me/learning-profile — called once right after OAuth with what
// the browser collected before sign-in, and again on later edits.
pub async fn put_my_profile(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<learning_profile::UpsertProfileRequest>,
) -> Result<Json<learning_profile::LearningProfile>, AppError> {
    Ok(Json(learning_profile::upsert(&state.db, &ctx, body).await?))
}

// POST /me/learning-profile/complete-onboarding
pub async fn post_complete_onboarding(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<learning_profile::LearningProfile>, AppError> {
    Ok(Json(learning_profile::complete_onboarding(&state.db, &ctx).await?))
}
