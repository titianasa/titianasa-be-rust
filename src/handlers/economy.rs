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

// GET /subscriptions/tiers — the paywall catalogue (price + benefits).
// Static, so it needs no DB round-trip.
pub async fn get_subscription_tiers() -> Json<Vec<subscription::TierBenefit>> {
    Json(subscription::list_tiers())
}

// POST /subscriptions/checkout — replaces the old
// /subscriptions/subscribe, which activated a paid tier immediately
// with no payment at all. The tier is now only granted by order.rs's
// webhook once QRIS settles.
#[derive(Debug, serde::Serialize)]
pub struct SubscriptionCheckoutResponse {
    pub order: crate::services::order::OrderResponse,
    pub qris_payload: String,
}

pub async fn post_subscription_checkout(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<SubscribeRequest>,
) -> Result<(StatusCode, Json<SubscriptionCheckoutResponse>), AppError> {
    let result = crate::services::order::checkout_subscription(
        &state.db,
        &ctx,
        state.payment_provider.as_ref(),
        &body.tier,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(SubscriptionCheckoutResponse { order: result.order, qris_payload: result.qris_payload }),
    ))
}

// GET /orders/{id} — polled by the paywall while the QRIS payment
// settles out-of-band.
pub async fn get_order(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<crate::services::order::OrderResponse>, AppError> {
    Ok(Json(crate::services::order::get_own_order(&state.db, &ctx, id).await?))
}

// GET /me/entitlements
pub async fn get_my_entitlements(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<subscription::Entitlements>, AppError> {
    Ok(Json(subscription::get_entitlements(&state.db, &ctx).await?))
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
