use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/assessments", post(handlers::assessment::post_assessment))
        .route("/assessments/{id}", get(handlers::assessment::get_assessment))
        .route("/assessments/{id}/attempts", post(handlers::assessment::post_attempt))
        .route("/assessments/{id}/exam-sessions", post(handlers::exam_session::post_exam_session))
        .route("/exam-sessions/{id}", get(handlers::exam_session::get_exam_session))
        .route("/lessons/{id}/attempts", post(handlers::assessment::post_lesson_attempt))
        .route("/attempts/{id}/submit", post(handlers::assessment::post_submit))
}
