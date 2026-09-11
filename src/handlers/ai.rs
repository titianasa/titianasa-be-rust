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
use crate::services::ai_provider::{default_ai_model, AI_MODEL_OPTIONS};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct AiModelOption {
    pub id: &'static str,
    pub label: &'static str,
}

#[derive(serde::Serialize)]
pub struct AiModelsResponse {
    pub models: Vec<AiModelOption>,
    pub default_model: &'static str,
}

// GET /ai/models — the allowlist an author may pick from when
// generating text/quiz content (Phase 38). Not a proxy of OpenRouter's
// full catalog — see AI_MODEL_OPTIONS for why it's curated.
pub async fn get_ai_models() -> Json<AiModelsResponse> {
    Json(AiModelsResponse {
        models: AI_MODEL_OPTIONS.iter().map(|(id, label)| AiModelOption { id, label }).collect(),
        default_model: default_ai_model(),
    })
}

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
        subject_id: body.subject_id,
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

// POST /ai/transcribe-audio — turns an uploaded recording into text an
// author can then build listening questions from, instead of typing the
// transcript by ear.
pub async fn post_transcribe_audio(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<crate::models::requests::ai::TranscribeAudioRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    use crate::services::drive_permissions::{resolve_access, DriveResource};
    use crate::services::permissions::{require_permission, Action, Resource};

    // Authoring action, gated like other content creation.
    require_permission(&ctx, Resource::ModuleItem, Action::Create)?;

    // The asset has to be one this user may actually read.
    if resolve_access(&state.db, &ctx, DriveResource::Asset, body.asset_id).await?.is_none() {
        return Err(AppError::NotFound("asset_not_found"));
    }
    let asset = crate::services::asset::find_by_id(&state.db, body.asset_id)
        .await?
        .ok_or(AppError::NotFound("asset_not_found"))?;
    if !asset.r#type.starts_with("audio/") && !asset.r#type.starts_with("video/") {
        return Err(AppError::UnprocessableEntity("not_an_audio_asset", "asset must be audio or video".to_string()));
    }

    let object = state
        .storage
        .get(&asset.id.to_string())
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("failed to read asset: {e}")))?;

    match state.ai_provider.transcribe(&object.bytes, &object.content_type, &state.config.ai_stt_model).await {
        Ok(result) => Ok(Json(serde_json::json!({ "transcript": result.text }))),
        Err(e) => {
            tracing::warn!(error = ?e, asset_id = %body.asset_id, "audio transcription failed");
            Err(AppError::UnprocessableEntity("transcription_failed", "gagal membuat transkrip dari audio ini".to_string()))
        }
    }
}

// POST /ai/generate-quiz-group
pub async fn post_generate_quiz_group(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<crate::models::requests::ai::GenerateQuizGroupRequest>,
) -> Result<Json<crate::services::quiz_generation::QuizGenerationResponse>, AppError> {
    use crate::services::quiz_generation::{generate_quiz_group, QuizGenerationBlueprint};
    // Extracting questions from a photographed page needs a
    // vision-capable model; the text-generation model has no image
    // endpoint at all and the provider rejects the call outright — an
    // author's model override only applies to the text path, not this.
    let model = if body.asset_id.is_some() {
        state.config.ai_ocr_model.as_str()
    } else {
        crate::services::ai_provider::resolve_ai_model(body.model.as_deref(), &state.config.ai_lesson_generation_model)?
    };
    let blueprint = QuizGenerationBlueprint {
        item_id: body.item_id,
        group_id: body.group_id,
        mode: crate::services::quiz_generation::GenerationMode::parse(body.mode.as_deref())?,
        question_number: body.question_number,
        count: body.count,
        context_prompt: body.context_prompt,
        reference_module_item_ids: body.reference_module_item_ids,
        asset_id: body.asset_id,
    };
    let result = generate_quiz_group(
        &state.db,
        &state.config,
        state.ai_provider.as_ref(),
        state.storage.as_ref(),
        &ctx,
        model,
        blueprint,
    )
    .await?;
    Ok(Json(result))
}
