use axum::{
    routing::{get, patch, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        // "/learning-paths/sync" and "/validate" sit before "{id}/sync"
        // so axum does not try to parse "sync" as a uuid.
        .route("/learning-paths/sync", post(handlers::module::post_sync_learning_paths))
        .route("/learning-paths/validate", get(handlers::module::get_learning_path_validation))
        .route("/learning-paths/{id}/sync", post(handlers::module::post_sync_learning_path))

        // GET /subjects — every Content Studio authoring form needs
        // this to offer a real subject picker.
        .route("/subjects", get(handlers::subject::get_subjects).post(handlers::subject::post_subject))

        // GET /quiz-subtypes — one source of truth for the 40-subtype
        // registry, consumed by the Quiz Builder's picker.
        .route("/quiz-subtypes", get(handlers::module::get_quiz_subtypes))

        // Exam blueprints — list, and apply one into a fresh quiz_config.
        .route("/quiz-taxonomy", get(handlers::module::get_quiz_taxonomy))
        .route("/quiz-templates", get(handlers::module::get_quiz_templates))
        .route("/quiz-templates/{id}", get(handlers::module::get_quiz_template))
        .route("/quiz-templates/{id}/apply", post(handlers::module::post_apply_quiz_template))

        .route("/modules", get(handlers::module::get_children).post(handlers::module::post_module))
        // Curriculum labels (migration 0039). The literal-segment routes
        // sit before "/modules/{id}" so axum doesn't match "by-label"
        // as an id.
        .route("/modules/by-label", get(handlers::module::get_modules_by_label))
        .route("/modules/label-rollup", get(handlers::module::get_label_rollup))
        .route("/module-labels/summary", get(handlers::module::get_label_summary))
        .route("/modules/{id}/labels", get(handlers::module::get_module_labels).post(handlers::module::post_module_label))
        .route("/modules/{id}/labels/{label_id}", axum::routing::delete(handlers::module::delete_module_label))
        .route("/modules/{id}", get(handlers::module::get_module).patch(handlers::module::patch_module).delete(handlers::module::delete_module))
        .route("/modules/{id}/ancestors", get(handlers::module::get_ancestors))
        .route("/modules/reorder", post(handlers::module::post_reorder_modules))
        .route("/modules/{id}/prerequisites", get(handlers::module::get_prerequisites).post(handlers::module::post_prerequisite))
        .route("/modules/{id}/prerequisites/{prerequisite_module_id}", axum::routing::delete(handlers::module::delete_prerequisite))
        .route("/modules/{id}/items", get(handlers::module::get_items).post(handlers::module::post_item))
        .route("/module-items/reorder", post(handlers::module::post_reorder_items))
        .route(
            "/module-items/{id}",
            get(handlers::module::get_item)
                .patch(handlers::module::patch_item)
                .put(handlers::module::put_item)
                .delete(handlers::module::delete_item),
        )
        .route("/module-items/{id}/quiz-config", patch(handlers::module::patch_quiz_config))
        .route("/module-items/{id}/lesson-plan", patch(handlers::module::patch_lesson_plan))
        // Aturan Akses & Guard, Attendance Guard, Proctor (migration 0045).
        .route("/module-items/{id}/guards", patch(handlers::item_rules::patch_guards))
        .route("/module-items/{id}/complete", post(handlers::item_rules::post_complete))
        .route("/content-reports", post(handlers::content_report::post_report))
        .route("/me/learning-heatmap", get(handlers::learning_heatmap::get_my_heatmap))
        .route("/module-items/{id}/checkpoints", get(handlers::item_rules::get_checkpoints))
        .route("/module-items/{id}/sections/{section_id}/checkpoint", post(handlers::item_rules::post_checkpoint))
        .route("/module-items/{id}/sections/{section_id}/reread", post(handlers::item_rules::post_checkpoint_reread))
        .route("/module-items/{id}/sections/{section_id}/checkpoint/preview", post(handlers::item_rules::post_checkpoint_preview))
        .route("/module-items/{id}/sections/{section_id}/checkpoint/preview/grade", post(handlers::item_rules::post_checkpoint_preview_grade))
        .route("/module-items/{id}/progress", get(handlers::item_rules::get_progress))
        .route("/module-items/{id}/progress/{user_id}/approve", post(handlers::item_rules::post_approval))
        .route(
            "/module-items/{id}/proctor-config",
            get(handlers::item_rules::get_proctor_config).put(handlers::item_rules::put_proctor_config),
        )
        .route("/module-items/{id}/effective-proctor-config", get(handlers::item_rules::get_effective_proctor_config))
        .route(
            "/module-items/{id}/proctor-sessions",
            get(handlers::item_rules::get_proctor_sessions).post(handlers::item_rules::post_proctor_session),
        )
        .route(
            "/modules/{id}/proctor-config",
            get(handlers::item_rules::get_module_proctor_config).put(handlers::item_rules::put_module_proctor_config),
        )
        .route(
            "/quiz-proctor-sessions/{id}/events",
            get(handlers::item_rules::get_proctor_events).post(handlers::item_rules::post_proctor_event),
        )
        .route("/quiz-proctor-sessions/{id}/unlock", post(handlers::item_rules::post_proctor_unlock))
        .route("/module-items/{id}/subject", patch(handlers::module::patch_item_subject))
        .route("/module-items/{id}/pending-reviews", get(handlers::module::get_pending_reviews))
        .route("/module-items/{id}/versions", get(handlers::module::get_item_versions))
        .route("/module-items/{id}/versions/{version}", get(handlers::module::get_item_version))
        .route("/module-items/{id}/duplicate", post(handlers::module::post_duplicate_item))
        .route("/module-items/{id}/submit-review", post(handlers::module::post_submit_review))
        .route("/module-items/{id}/publish", post(handlers::module::post_publish))
        .route("/module-items/{id}/reject", post(handlers::module::post_reject))
        .route("/module-items/{id}/speaking-prompt-audio", get(handlers::module::get_speaking_prompt_audio))
        .route("/module-items/{id}/completion-status", get(handlers::module::get_completion_status))
        .route("/module-items/{id}/skip-completion", post(handlers::module::post_skip_completion))
        .route("/module-items/{id}/shares", get(handlers::module::get_item_shares).post(handlers::module::post_item_share))
        .route("/module-items/{id}/shares/{share_id}", axum::routing::delete(handlers::module::delete_item_share))
}

// The collab-editing WebSocket does its OWN auth inline (query-param
// token, not a header — browsers can't set custom headers on a WS
// handshake) and must NOT sit behind the blanket auth_middleware layer
// — same reasoning and pattern as routes/messaging.rs's canvas WS.
pub fn public_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ws/module-items/{item_id}/collab", get(handlers::collab_ws::ws_handler))
        .route("/public/curriculum-preview", get(handlers::module::get_curriculum_preview))
        // sit before "/public/learning-paths" is not needed (distinct
        // literal segments), but keep them adjacent for readability.
        .route("/public/learning-paths", get(handlers::module::get_landing_paths))
        .route("/public/learning-paths/children", get(handlers::module::get_landing_children))
}
