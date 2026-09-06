use axum::{routing::post, Router};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// AI Gateway (P1-011/P2-013/P2-015).
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ai/evaluate", post(handlers::ai::post_evaluate))
        .route("/ai/generate-lesson", post(handlers::ai::post_generate_lesson))
        .route("/ai/generate-questions", post(handlers::ai::post_generate_questions))
        .route("/ai/ocr-to-question", post(handlers::ai::post_ocr_to_question))
}
