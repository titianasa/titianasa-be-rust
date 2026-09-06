use axum::{routing::get, routing::post, Router};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/programs", get(handlers::program::get_programs).post(handlers::program::post_program))
        .route("/programs/{id}", get(handlers::program::get_program).patch(handlers::program::patch_program))
        .route("/programs/{id}/tree", get(handlers::program::get_tree))
        .route("/programs/{id}/modules", post(handlers::program::post_attach_module))
        .route("/programs/{id}/modules/{module_id}", axum::routing::delete(handlers::program::delete_module))
        .route("/programs/{id}/modules/reorder", post(handlers::program::post_reorder_modules))
}
