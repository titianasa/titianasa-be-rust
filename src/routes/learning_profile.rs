use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/me/learning-profile",
            get(handlers::learning_profile::get_my_profile).put(handlers::learning_profile::put_my_profile),
        )
        .route(
            "/me/learning-profile/complete-onboarding",
            post(handlers::learning_profile::post_complete_onboarding),
        )
}
