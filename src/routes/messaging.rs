use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// Messaging (P22-001) + Canvas REST (P26-001) — every route here
// requires a bearer token via the normal auth_middleware layer.
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cohorts/{id}/conversations", post(handlers::messaging::post_open_conversation))
        .route("/conversations", get(handlers::messaging::get_conversations))
        .route("/conversations/{id}/messages", get(handlers::messaging::get_messages).post(handlers::messaging::post_message))
        .route("/conversations/{id}/read", post(handlers::messaging::post_mark_read))
        .route("/conversations/{id}/canvas-sessions", post(handlers::canvas::post_canvas_session).get(handlers::canvas::get_canvas_sessions))
        .route("/canvas-sessions/{id}", get(handlers::canvas::get_canvas_session))
        .route("/canvas-sessions/{id}/submit", post(handlers::canvas::post_submit))
}

// The canvas collaborative-editing WebSocket does its OWN auth inline
// (query-param token, not a header — browsers can't set custom headers
// on a WS handshake) and must NOT sit behind the blanket auth_middleware
// layer, matching the Bun original's `.ws()` registration being outside
// its derived-auth-context HTTP request flow entirely.
pub fn public_routes() -> Router<Arc<AppState>> {
    Router::new().route("/ws/canvas/{session_id}", get(handlers::canvas_ws::ws_handler))
}
