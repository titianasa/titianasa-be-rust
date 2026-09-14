use axum::{
    routing::{get, patch, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/classes", get(handlers::org_class::get_classes).post(handlers::org_class::post_class))
        .route("/classes/{id}", get(handlers::org_class::get_class).patch(handlers::org_class::patch_class))
        .route("/classes/{id}/members", post(handlers::org_class::post_class_member))
        .route("/classes/{id}/learning-heatmap", get(handlers::learning_heatmap::get_class_heatmap))
        .route("/classes/{id}/members/{student_id}", axum::routing::delete(handlers::org_class::delete_class_member))
        // Phase 35 — sessions + attendance.
        .route("/classes/{id}/sessions", get(handlers::org_class_session::get_sessions).post(handlers::org_class_session::post_session))
        .route("/org-class-sessions/{id}", get(handlers::org_class_session::get_session))
        .route("/org-class-sessions/{id}/sync-attendance", post(handlers::org_class_session::post_sync_attendance))
        .route("/org-class-sessions/{id}/attendance/scan", post(handlers::org_attendance::post_scan))
        .route("/org-class-sessions/{id}/attendance/self-check-in", post(handlers::org_attendance::post_self_check_in))
        .route("/org-class-sessions/{id}/gate-status", get(handlers::item_rules::get_gate_status))
        .route("/classes/{id}/attendance/{date}", get(handlers::org_attendance::get_attendance).post(handlers::org_attendance::post_attendance))
        .route("/me/attendance-qr-token", get(handlers::org_attendance::get_my_qr_token))
        // Phase 36 — Periode.
        .route("/organizations/{id}/periods", get(handlers::period::get_periods).post(handlers::period::post_period))
        .route("/periods/{id}", patch(handlers::period::patch_period))
}
