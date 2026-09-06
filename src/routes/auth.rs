use axum::{routing::get, routing::post, Router};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// No auth required — these two ARE the auth flow.
pub fn public_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/google/callback", post(handlers::auth::google_callback))
        .route("/auth/refresh", post(handlers::auth::refresh))
}

// Merged into routes/mod.rs's single protected subtree, which attaches
// auth_middleware once for all feature modules combined — not per
// module, to avoid layering the same middleware N times.
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new().route("/users/me", get(handlers::auth::get_me))
}
