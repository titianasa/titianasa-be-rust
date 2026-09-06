use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::order::{self, OrderResponse};
use crate::services::wallet::{self, WalletResponse};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct CheckoutResponse {
    #[serde(flatten)]
    pub order: OrderResponse,
    pub qris_payload: String,
}

// POST /enrollments/{id}/checkout
pub async fn post_checkout(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(enrollment_id): Path<Uuid>,
) -> Result<(StatusCode, Json<CheckoutResponse>), AppError> {
    let result = order::checkout(&state.db, &ctx, state.payment_provider.as_ref(), enrollment_id).await?;
    Ok((StatusCode::CREATED, Json(CheckoutResponse { order: result.order, qris_payload: result.qris_payload })))
}

// POST /payments/{payment_id}/webhook — PUBLIC, no auth extractor.
pub async fn post_webhook(State(state): State<Arc<AppState>>, Path(payment_id): Path<String>) -> Result<Json<OrderResponse>, AppError> {
    Ok(Json(order::handle_webhook(&state.db, &payment_id).await?))
}

// GET /tutors/me/wallet
pub async fn get_wallet(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<WalletResponse>, AppError> {
    Ok(Json(wallet::get_wallet(&state.db, &ctx).await?))
}
