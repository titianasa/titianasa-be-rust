use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// Class sessions + attendance + attendance-verification (R8). All
// protected — no route here is public.
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cohorts/{id}/sessions/{session_date}/attendance", post(handlers::attendance::post_attendance))
        .route("/cohorts/{id}/attendance", get(handlers::attendance::get_attendance))
        .route("/cohorts/{id}/class-sessions", post(handlers::class_session::post_class_session).get(handlers::class_session::get_class_sessions))
        .route("/class-sessions/{id}", get(handlers::class_session::get_class_session))
        .route("/class-sessions/{id}/simulate-participant", post(handlers::class_session::post_simulate_participant))
        .route("/class-sessions/{id}/sync-attendance", post(handlers::class_session::post_sync_attendance))
        .route("/class-sessions/{id}/attendance", get(handlers::class_session::get_session_attendance))
        .route("/class-sessions/{id}/recording", get(handlers::class_session::get_recording))
}
