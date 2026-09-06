use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/modules", get(handlers::module::get_children).post(handlers::module::post_module))
        .route("/modules/{id}", get(handlers::module::get_module).patch(handlers::module::patch_module))
        .route("/modules/{id}/ancestors", get(handlers::module::get_ancestors))
        .route("/modules/reorder", post(handlers::module::post_reorder_modules))
        .route("/modules/{id}/prerequisites", get(handlers::module::get_prerequisites).post(handlers::module::post_prerequisite))
        .route("/modules/{id}/prerequisites/{prerequisite_module_id}", axum::routing::delete(handlers::module::delete_prerequisite))
        .route("/modules/{id}/items", get(handlers::module::get_items).post(handlers::module::post_item))
        .route("/module-items/reorder", post(handlers::module::post_reorder_items))
        .route("/module-items/{id}", get(handlers::module::get_item).patch(handlers::module::patch_item).put(handlers::module::put_item))
        .route("/module-items/{id}/submit-review", post(handlers::module::post_submit_review))
        .route("/module-items/{id}/publish", post(handlers::module::post_publish))
        .route("/module-items/{id}/reject", post(handlers::module::post_reject))
        .route("/module-items/{id}/speaking-prompt-audio", get(handlers::module::get_speaking_prompt_audio))
        .route("/module-items/{id}/completion-status", get(handlers::module::get_completion_status))
        .route("/module-items/{id}/skip-completion", post(handlers::module::post_skip_completion))
}
