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
        .route("/subscriptions/subscribe", post(handlers::economy::post_subscribe))
        .route("/subscriptions/me", get(handlers::economy::get_my_subscription))
        .route("/ads/watch", post(handlers::economy::post_watch_ad))
}
