use axum::{extract::State, http::StatusCode, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::economy::SubscribeRequest;
use crate::services::{ad, economy, subscription};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct CreditsResponse {
    pub balance: i64,
}

// GET /me/credits
pub async fn get_my_credits(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<CreditsResponse>, AppError> {
    let balance = economy::get_my_balance(&state.db, &ctx).await?;
    Ok(Json(CreditsResponse { balance }))
}

// POST /subscriptions/subscribe
pub async fn post_subscribe(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<SubscribeRequest>,
) -> Result<(StatusCode, Json<subscription::SubscriptionResponse>), AppError> {
    let result = subscription::subscribe(&state.db, &ctx, &body.tier).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /subscriptions/me
pub async fn get_my_subscription(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<subscription::SubscriptionResponse>, AppError> {
    Ok(Json(subscription::get_my_subscription(&state.db, &ctx).await?))
}

// POST /ads/watch
pub async fn post_watch_ad(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<(StatusCode, Json<ad::WatchAdResponse>), AppError> {
    let result = ad::watch_ad(&state.db, &ctx).await?;
    Ok((StatusCode::CREATED, Json(result)))
}
