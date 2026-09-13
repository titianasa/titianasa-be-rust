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

#[derive(serde::Serialize)]
pub struct AiModelOption {
    pub id: String,
    pub label: String,
}

#[derive(serde::Serialize)]
pub struct AiModelsResponse {
    pub models: Vec<AiModelOption>,
    pub default_model: String,
}

// GET /ai/models — the allowlist an author may pick from when
// generating text/quiz content (Phase 38). P40-003: now the AI model
// catalog's enabled, text-capable rows (Pengaturan AI), not a hardcoded
// const — an admin toggling a model off removes it from this list on
// the next call. `default_model` is `lesson_generation`'s resolved
// model (the role every one of these call sites' `body.model: None`
// ultimately falls through to today).
pub async fn get_ai_models(State(state): State<Arc<AppState>>) -> Result<Json<AiModelsResponse>, AppError> {
    let options = crate::services::ai_settings::text_capable_models(&state.db).await?;
    let default_model = crate::services::ai_settings::resolve(&state.db, &state.config, "lesson_generation").await?.model_id;
    Ok(Json(AiModelsResponse { models: options.into_iter().map(|c| AiModelOption { id: c.model_id, label: c.label }).collect(), default_model }))
}

// POST /ai/evaluate
pub async fn post_evaluate(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<EvaluateRequest>) -> Result<Json<EvaluateResponse>, AppError> {
    let result = ai_gateway::evaluate(&state.db, &state.config, state.text_ai_provider.as_ref(), ctx.user_id, &body.task, &body.input).await?;
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
    let model = crate::services::ai_settings::resolve(&state.db, &state.config, "lesson_generation").await?.model_id;
    let result = ai_content::generate_lesson(&state.db, &ctx, state.text_ai_provider.as_ref(), &model, blueprint).await?;
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
    let model = crate::services::ai_settings::resolve(&state.db, &state.config, "question_generation").await?.model_id;
    let result = ai_content::generate_questions(&state.db, &ctx, state.text_ai_provider.as_ref(), &model, blueprint).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /ai/ocr-to-question — P2-015. Auth: curriculum_developer+ (same
// gate as POST /question-banks/{id}/questions).
pub async fn post_ocr_to_question(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<OcrToQuestionRequest>) -> Result<(StatusCode, Json<OcrToQuestionResponse>), AppError> {
    let blueprint = OcrToQuestionBlueprint { bank_id: body.bank_id, asset_id: body.asset_id, concept_ids: body.concept_ids.unwrap_or_default() };
    let model = crate::services::ai_settings::resolve(&state.db, &state.config, "ocr").await?.model_id;
    let result = ai_ocr::ocr_to_question(&state.db, &state.config, &ctx, state.text_ai_provider.as_ref(), &model, state.storage.as_ref(), blueprint).await?;
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

    let model = crate::services::ai_settings::resolve(&state.db, &state.config, "stt").await?.model_id;
    match state.ai_provider.transcribe(&object.bytes, &object.content_type, &model).await {
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
    // Vertex AI Gemini (`text_ai_provider`) handles vision natively —
    // OCR and text generation are the SAME provider now, just a
    // different model slot. An author's model override only applies to
    // the text path, not OCR: extracting from a photographed page needs
    // a specific vision-capable model, not whatever the author picked.
    let model = if body.asset_id.is_some() {
        crate::services::ai_settings::resolve(&state.db, &state.config, "ocr").await?.model_id
    } else {
        crate::services::ai_provider::resolve_ai_model(&state.db, &state.config, "quiz_generation", body.model.as_deref()).await?
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
        raw_text: body.raw_text,
        convert_to_subtype: None,
        mark_draft: body.mark_draft,
    };
    let result = generate_quiz_group(
        &state.db,
        &state.config,
        state.text_ai_provider.as_ref(),
        state.storage.as_ref(),
        &ctx,
        &model,
        blueprint,
    )
    .await?;
    Ok(Json(result))
}

// POST /ai/quiz/generate-batch
pub async fn post_generate_quiz_batch(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<crate::models::requests::ai::GenerateQuizBatchRequest>,
) -> Result<Json<crate::services::quiz_generation::BatchGenerationResponse>, AppError> {
    use crate::services::quiz_generation::generate_batch;
    let model = crate::services::ai_provider::resolve_ai_model(&state.db, &state.config, "quiz_generation", body.model.as_deref()).await?;
    let result = generate_batch(
        &state.db,
        &state.config,
        state.text_ai_provider.as_ref(),
        state.storage.as_ref(),
        &ctx,
        &model,
        body.item_id,
        body.count,
    )
    .await?;
    Ok(Json(result))
}

// POST /ai/quiz/convert-group-type
pub async fn post_convert_group_type(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<crate::models::requests::ai::ConvertGroupTypeRequest>,
) -> Result<Json<crate::services::quiz_generation::QuizGenerationResponse>, AppError> {
    use crate::services::quiz_generation::convert_group_type;
    let model = crate::services::ai_provider::resolve_ai_model(&state.db, &state.config, "quiz_generation", body.model.as_deref()).await?;
    let result = convert_group_type(
        &state.db,
        &state.config,
        state.text_ai_provider.as_ref(),
        state.storage.as_ref(),
        &ctx,
        &model,
        body.item_id,
        body.group_id,
        body.new_subtype,
    )
    .await?;
    Ok(Json(result))
}

// POST /ai/quiz/suggest-group-types
pub async fn post_suggest_group_types(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<crate::models::requests::ai::SuggestGroupTypesRequest>,
) -> Result<Json<crate::services::quiz_generation::SuggestGroupTypesResponse>, AppError> {
    use crate::services::quiz_generation::suggest_group_types;
    let model = crate::services::ai_provider::resolve_ai_model(&state.db, &state.config, "quiz_generation", body.model.as_deref()).await?;
    let result = suggest_group_types(&state.db, state.text_ai_provider.as_ref(), &ctx, &model, &body.document_text).await?;
    Ok(Json(result))
}
