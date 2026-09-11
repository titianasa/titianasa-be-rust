use axum::{routing::{get, post}, Router};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// AI Gateway (P1-011/P2-013/P2-015).
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ai/models", get(handlers::ai::get_ai_models))
        .route("/ai/evaluate", post(handlers::ai::post_evaluate))
        .route("/ai/generate-lesson", post(handlers::ai::post_generate_lesson))
        .route("/ai/generate-questions", post(handlers::ai::post_generate_questions))
        .route("/ai/ocr-to-question", post(handlers::ai::post_ocr_to_question))
        .route("/ai/transcribe-audio", post(handlers::ai::post_transcribe_audio))
        .route("/ai/generate-quiz-group", post(handlers::ai::post_generate_quiz_group))
        .route("/ai/generate-lesson-plan", post(handlers::lesson_plan::post_generate_lesson_plan))
        .route("/ai/edit-lesson-section", post(handlers::lesson_plan::post_edit_lesson_section))
        .route("/ai/translate-lesson-plan", post(handlers::lesson_plan::post_translate_lesson_plan))
        // Axum's 2 MB default body limit would reject most real PDFs.
        .route(
            "/documents/extract",
            post(handlers::document::post_extract_document).layer(axum::extract::DefaultBodyLimit::max(12 * 1024 * 1024)),
        )
        .route("/ai/live-chat-turn", post(handlers::live_chat::post_live_chat_turn))
        .route("/ai/generate-alm-fragment", post(handlers::alm_generation::post_generate_fragment))
}
