use axum::{extract::State, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::ai::{EditLessonSectionRequest, GenerateLessonPlanRequest, TranslateLessonPlanRequest};
use crate::services::ai_provider::resolve_ai_model;
use crate::services::lesson_plan_ai::{self, EditSectionResponse, GeneratePlanResponse, TranslatePlanResponse};
use crate::state::AppState;

// POST /ai/generate-lesson-plan — topic → a whole Modul Belajar, not
// saved (the editor applies it as an unsaved change).
pub async fn post_generate_lesson_plan(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<GenerateLessonPlanRequest>,
) -> Result<Json<GeneratePlanResponse>, AppError> {
    // Cloned rather than borrowed from `body`: `body` moves whole into
    // `generate_plan` below, so `model` can't still be borrowing from it.
    let requested_model = body.model.clone();
    let model = resolve_ai_model(&state.db, &state.config, "lesson_generation", requested_model.as_deref()).await?;
    Ok(Json(lesson_plan_ai::generate_plan(&state.db, state.text_ai_provider.as_ref(), &model, &ctx, body).await?))
}

// POST /ai/edit-lesson-section — one section rewritten (or written from
// scratch) from a natural-language instruction.
pub async fn post_edit_lesson_section(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<EditLessonSectionRequest>,
) -> Result<Json<EditSectionResponse>, AppError> {
    let model = crate::services::ai_settings::resolve(&state.db, &state.config, "lesson_generation").await?.model_id;
    Ok(Json(lesson_plan_ai::edit_section(&state.db, state.text_ai_provider.as_ref(), &model, &ctx, body).await?))
}

// POST /ai/translate-lesson-plan — the learner's chosen reading language.
pub async fn post_translate_lesson_plan(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<TranslateLessonPlanRequest>,
) -> Result<Json<TranslatePlanResponse>, AppError> {
    let model = crate::services::ai_settings::resolve(&state.db, &state.config, "lesson_generation").await?.model_id;
    Ok(Json(lesson_plan_ai::translate_plan(&state.db, state.text_ai_provider.as_ref(), &model, &ctx, body).await?))
}
