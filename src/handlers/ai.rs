use axum::{
    extract::State,
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::ai::{EvaluateRequest, GenerateLessonRequest, GenerateQuestionsRequest, OcrToQuestionRequest};
use crate::services::ai_content::{self, GenerateLessonResponse, GenerateQuestionsResponse, LessonGenerationBlueprint, QuestionGenerationBlueprint};
use crate::services::ai_gateway::{self, EvaluateResponse};
use crate::services::ai_ocr::{self, OcrToQuestionBlueprint, OcrToQuestionResponse};
use crate::state::AppState;

// POST /ai/evaluate
pub async fn post_evaluate(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<EvaluateRequest>) -> Result<Json<EvaluateResponse>, AppError> {
    let result = ai_gateway::evaluate(&state.db, &state.config, state.ai_provider.as_ref(), ctx.user_id, &body.task, &body.input).await?;
    Ok(Json(result))
}

// POST /ai/generate-lesson — P2-013. Auth: curriculum_developer+ (same
// gate as POST /lessons).
pub async fn post_generate_lesson(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<GenerateLessonRequest>) -> Result<(StatusCode, Json<GenerateLessonResponse>), AppError> {
    let blueprint = LessonGenerationBlueprint {
        module_id: body.module_id,
        parent_id: body.parent_id,
        content_type: body.content_type,
        topic: body.topic,
        grammar_target: body.grammar_target,
        vocab_target: body.vocab_target.unwrap_or_default(),
        concept_ids: body.concept_ids.unwrap_or_default(),
    };
    let result = ai_content::generate_lesson(&state.db, &ctx, state.ai_provider.as_ref(), &state.config.ai_lesson_generation_model, blueprint).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /ai/generate-questions — P2-013. Auth: curriculum_developer+
// (same gate as POST /question-banks/{id}/questions).
pub async fn post_generate_questions(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<GenerateQuestionsRequest>,
) -> Result<(StatusCode, Json<GenerateQuestionsResponse>), AppError> {
    let blueprint = QuestionGenerationBlueprint { bank_id: body.bank_id, question_type: body.question_type, topic: body.topic, count: body.count, difficulty: body.difficulty, concept_ids: body.concept_ids.unwrap_or_default() };
    let result = ai_content::generate_questions(&state.db, &ctx, state.ai_provider.as_ref(), &state.config.ai_question_generation_model, blueprint).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /ai/ocr-to-question — P2-015. Auth: curriculum_developer+ (same
// gate as POST /question-banks/{id}/questions).
pub async fn post_ocr_to_question(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<OcrToQuestionRequest>) -> Result<(StatusCode, Json<OcrToQuestionResponse>), AppError> {
    let blueprint = OcrToQuestionBlueprint { bank_id: body.bank_id, asset_id: body.asset_id, concept_ids: body.concept_ids.unwrap_or_default() };
    let result = ai_ocr::ocr_to_question(&state.db, &state.config, &ctx, state.ai_provider.as_ref(), &state.config.ai_ocr_model, state.storage.as_ref(), blueprint).await?;
    Ok((StatusCode::CREATED, Json(result)))
}
