use axum::{extract::State, http::HeaderMap, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::auth::{GoogleCallbackRequest, RefreshRequest};
use crate::models::responses::auth::{LoginResponse, MeResponse, RefreshResponse};
use crate::services::request_meta::MaybeConnectInfo;
use crate::services::{auth as auth_service, request_meta};
use crate::state::AppState;

// POST /auth/google/callback
pub async fn google_callback(
    State(state): State<Arc<AppState>>,
    MaybeConnectInfo(addr): MaybeConnectInfo,
    headers: HeaderMap,
    ValidatedJson(body): ValidatedJson<GoogleCallbackRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    let meta = request_meta::extract(&headers, addr.map(axum::extract::ConnectInfo).as_ref());
    let result =
        auth_service::google_callback(&state.db, &state.google_verifier, &state.config, &body.id_token, &meta)
            .await?;
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
