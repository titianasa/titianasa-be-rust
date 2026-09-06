use axum::{routing::get, routing::post, Router};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/question-banks", get(handlers::question::get_question_banks).post(handlers::question::post_question_bank))
        .route(
            "/question-banks/{id}/questions",
            post(handlers::question::post_question).get(handlers::question::get_questions),
        )
        .route("/questions/{id}", get(handlers::question::get_question))
        .route("/questions/{id}/stem", get(handlers::question::get_question_stem))
        .route("/questions/{id}/submit-review", post(handlers::question::post_submit_review))
        .route("/questions/{id}/publish", post(handlers::question::post_publish))
        .route("/questions/{id}/reject", post(handlers::question::post_reject))
        .route("/questions/{id}/check", post(handlers::question::post_check))
}
