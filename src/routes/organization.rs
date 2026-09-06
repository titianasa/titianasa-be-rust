use axum::{
    routing::{get, patch, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// All org/tutor routes need auth (either a role check inside the
// service, or ownership via ctx.user_id) — merged into
// routes/auth.rs's protected_routes subtree by the caller.
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/organizations/{id}/members", get(handlers::organization::get_members))
        .route(
            "/organizations/{id}/tutors",
            post(handlers::tutor::post_tutor).get(handlers::tutor::get_tutors),
        )
        .route("/tutors/me", patch(handlers::tutor::patch_own_profile))
}
