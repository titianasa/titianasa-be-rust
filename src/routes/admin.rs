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
        .route("/admin/ai/catalog", get(handlers::admin::get_ai_catalog).post(handlers::admin::post_ai_catalog))
        .route("/admin/ai/catalog/{id}", patch(handlers::admin::patch_ai_catalog))
        .route("/admin/ai/roles", get(handlers::admin::get_ai_roles))
        .route("/admin/ai/roles/{role}", put(handlers::admin::put_ai_role))
        .route("/admin/ai/roles/{role}/test", post(handlers::admin::post_ai_role_test))
        .route("/admin/metrics/ringkasan", get(handlers::admin::get_metrics_ringkasan))
        .route("/admin/metrics/penjualan", get(handlers::admin::get_metrics_penjualan))
        .route("/admin/metrics/kurikulum", get(handlers::admin::get_metrics_kurikulum))
        .route("/admin/metrics/operasional-ai", get(handlers::admin::get_metrics_operasional_ai))
        .route("/admin/metrics/organisasi", get(handlers::admin::get_metrics_organisasi))
        .route("/admin/metrics/peserta", get(handlers::admin::get_metrics_peserta))
        .route("/admin/participants/search", get(handlers::admin::search_participants))
        .route("/admin/participants/{user_id}", get(handlers::admin::get_participant_detail))
        .route("/admin/content-tickets", get(handlers::content_report::get_tickets))
        .route("/admin/learning/heatmap", get(handlers::learning_heatmap::get_platform_heatmap))
        .route("/admin/content-tickets/{id}", get(handlers::content_report::get_ticket).patch(handlers::content_report::patch_ticket))
        .route("/admin/content-factory/coverage", get(handlers::content_factory::get_coverage))
        .route("/admin/content-factory/tahap/{id}", get(handlers::content_factory::get_tahap_tree))
        .route("/admin/content-factory/runs", get(handlers::content_factory::get_runs).post(handlers::content_factory::post_run))
        .route("/admin/content-factory/runs/{id}", get(handlers::content_factory::get_run))
        .route("/admin/content-factory/runs/{id}/action", post(handlers::content_factory::post_run_action))
        .route("/admin/content-factory/review", get(handlers::content_factory::get_review_queue))
        .route("/admin/content-factory/tasks/{id}", get(handlers::content_factory::get_task))
        .route("/admin/content-factory/tasks/{id}/review", post(handlers::content_factory::post_task_review))
        .route("/admin/content-factory/standards", get(handlers::content_factory::get_standards))
        .route("/admin/content-factory/standards/{tahap_id}", put(handlers::content_factory::put_standard))
        .route("/admin/content-factory/exemplars", get(handlers::content_factory::get_exemplars))
        .route("/admin/content-factory/seed", post(handlers::content_factory::post_seed))
}
