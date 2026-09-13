use axum::{
    routing::{get, patch, post, put},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/audit-log", get(handlers::admin::get_audit_log))
        .route("/admin/ai/catalog", get(handlers::admin::get_ai_catalog))
        .route("/admin/ai/catalog/{id}", patch(handlers::admin::patch_ai_catalog))
        .route("/admin/ai/roles", get(handlers::admin::get_ai_roles))
        .route("/admin/ai/roles/{role}", put(handlers::admin::put_ai_role))
        .route("/admin/ai/roles/{role}/test", post(handlers::admin::post_ai_role_test))
        .route("/admin/metrics/ringkasan", get(handlers::admin::get_metrics_ringkasan))
        .route("/admin/metrics/penjualan", get(handlers::admin::get_metrics_penjualan))
        .route("/admin/metrics/kurikulum", get(handlers::admin::get_metrics_kurikulum))
        .route("/admin/metrics/operasional-ai", get(handlers::admin::get_metrics_operasional_ai))
        .route("/admin/metrics/organisasi", get(handlers::admin::get_metrics_organisasi))
}
