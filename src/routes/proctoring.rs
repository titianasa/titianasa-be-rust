use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/proctoring-policies", post(handlers::proctoring_policy::post_policy).get(handlers::proctoring_policy::get_policies))
        .route("/exam-sessions/{id}/proctoring-session", post(handlers::proctoring::post_session))
        .route("/proctoring-sessions/{id}/events", post(handlers::proctoring::post_event))
        .route("/proctoring-sessions", get(handlers::proctoring::get_sessions))
        .route("/proctoring-sessions/{id}", get(handlers::proctoring::get_session))
        .route("/proctoring-sessions/{id}/review", post(handlers::proctoring::post_review))
}
