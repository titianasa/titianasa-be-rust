use axum::{extract::State, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::auth::{GoogleCallbackRequest, RefreshRequest};
use crate::models::responses::auth::{LoginResponse, MeResponse, RefreshResponse};
use crate::services::auth as auth_service;
use crate::state::AppState;

// POST /auth/google/callback
pub async fn google_callback(
    State(state): State<Arc<AppState>>,
    ValidatedJson(body): ValidatedJson<GoogleCallbackRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    let result =
        auth_service::google_callback(&state.db, &state.google_verifier, &state.config, &body.id_token).await?;
    Ok(Json(result))
}

// POST /auth/refresh
pub async fn refresh(
    State(state): State<Arc<AppState>>,
    ValidatedJson(body): ValidatedJson<RefreshRequest>,
) -> Result<Json<RefreshResponse>, AppError> {
    let result = auth_service::refresh(&state.db, &state.config, &body.refresh_token).await?;
    Ok(Json(result))
}

// GET /users/me
pub async fn get_me(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<MeResponse>, AppError> {
    let (user, roles) = auth_service::get_me(&state.db, ctx.user_id).await?;
    Ok(Json(MeResponse { id: user.id, email: user.email, name: user.name, roles }))
}
