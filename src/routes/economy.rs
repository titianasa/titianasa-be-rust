use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/me/credits", get(handlers::economy::get_my_credits))
        .route("/subscriptions/tiers", get(handlers::economy::get_subscription_tiers))
        .route("/subscriptions/checkout", post(handlers::economy::post_subscription_checkout))
        .route("/subscriptions/me", get(handlers::economy::get_my_subscription))
        .route("/me/entitlements", get(handlers::economy::get_my_entitlements))
        .route("/orders/{id}", get(handlers::economy::get_order))
        .route("/ads/watch", post(handlers::economy::post_watch_ad))
}
