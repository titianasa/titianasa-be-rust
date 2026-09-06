use axum::{
    routing::{delete, get},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/mastery/{concept_id}", get(handlers::learning::get_mastery))
        .route("/concepts/{concept_id}/mastery-breakdown", get(handlers::learning::get_mastery_breakdown))
        .route("/concepts/{concept_id}/rescue-status", get(handlers::learning::get_rescue_status))
        .route(
            "/concepts/{concept_id}/prerequisites",
            get(handlers::concept::get_prerequisites).post(handlers::concept::post_prerequisite),
        )
        .route("/concepts/{concept_id}/prerequisites/{prerequisite_concept_id}", delete(handlers::concept::delete_prerequisite))
        .route("/review-queue", get(handlers::learning::get_review_queue))
        .route("/learning-queue", get(handlers::learning::get_learning_queue))
}
