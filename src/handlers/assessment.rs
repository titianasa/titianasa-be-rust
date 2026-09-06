use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::assessment::{CreateAssessmentRequest, SubmitAttemptRequest};
use crate::models::responses::assessment::AssessmentSummary;
use crate::services::assessment::{self, CreateAttemptResponse, CreateLessonAttemptResponse};
use crate::services::{achievement, ai_speaking_evaluation, ai_writing_evaluation, daily_mission, exam_session, streak, xp};
use crate::state::AppState;

// POST /assessments
pub async fn post_assessment(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<CreateAssessmentRequest>,
) -> Result<(StatusCode, Json<AssessmentSummary>), AppError> {
    let new_assessment = assessment::NewAssessment {
        r#type: body.r#type,
        title: body.title,
        config: body.config.unwrap_or_else(|| serde_json::json!({})),
        question_ids: body.question_ids,
    };
    let result = assessment::create_assessment(&state.db, &ctx, new_assessment).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /assessments/{id}
pub async fn get_assessment(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
    Path(assessment_id): Path<Uuid>,
) -> Result<Json<AssessmentSummary>, AppError> {
    Ok(Json(assessment::get_assessment(&state.db, assessment_id).await?))
}

// POST /assessments/{id}/attempts
pub async fn post_attempt(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(assessment_id): Path<Uuid>,
) -> Result<(StatusCode, Json<CreateAttemptResponse>), AppError> {
    let result = assessment::create_attempt(&state.db, &ctx, assessment_id).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /lessons/{id}/attempts — P3-004/P6-002 (writing/speaking).
pub async fn post_lesson_attempt(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(item_id): Path<Uuid>,
) -> Result<(StatusCode, Json<CreateLessonAttemptResponse>), AppError> {
    let result = assessment::create_lesson_attempt(&state.db, &ctx, item_id).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /attempts/{attempt_id}/submit — branches on the attempt's own
// kind (assessment vs. lesson, and for a lesson, its type) exactly like
// assessment_handler.ts's postSubmit; the XP/streak/achievement/
// daily-mission/exam-session hooks are composed HERE, at the handler,
// not inside any service function, matching that file's own reasoning
// (avoids assessment_service depending on the AI evaluation services,
// which already depend on it).
pub async fn post_submit(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(attempt_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<SubmitAttemptRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let attempt = assessment::find_attempt(&state.db, attempt_id).await?.ok_or(AppError::NotFound("attempt_not_found"))?;

    if let Some(item_id) = attempt.item_id {
        let content_type = sqlx::query_scalar!(r#"select content_type from module_items where id = $1"#, item_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("attempt references a missing module item")))?;

        if content_type.as_deref() == Some("speaking") {
            let audio_asset_id = body
                .answer_audio_asset_id
                .ok_or_else(|| AppError::UnprocessableEntity("missing_answer_audio_asset_id", "answer_audio_asset_id is required to submit a speaking attempt".to_string()))?;
            let result = ai_speaking_evaluation::submit_speaking_attempt(&state.db, &state.config, state.ai_provider.as_ref(), state.storage.as_ref(), &ctx, attempt_id, audio_asset_id).await?;

            xp::award_xp(&state.db, ctx.user_id, xp::skill_xp("speaking").unwrap(), "speaking_completed", Some(&format!("attempt:{attempt_id}")), Some("speaking")).await?;
            streak::record_activity(&state.db, ctx.user_id, chrono::Utc::now()).await?;
            achievement::check_and_award(&state.db, ctx.user_id, &achievement::ActivityContext { skill_category: Some("speaking".to_string()), question_ids: vec![] }).await?;
            daily_mission::record_progress(&state.db, ctx.user_id, Some("speaking"), chrono::Utc::now()).await?;

            return Ok(Json(serde_json::to_value(result).map_err(|e| AppError::Internal(e.into()))?));
        }

        let answer_text = body
            .answer_text
            .ok_or_else(|| AppError::UnprocessableEntity("missing_answer_text", "answer_text is required to submit a writing attempt".to_string()))?;
        let result = ai_writing_evaluation::submit_writing_attempt(&state.db, state.ai_provider.as_ref(), &state.config.ai_writing_evaluation_model, &ctx, attempt_id, &answer_text).await?;

        xp::award_xp(&state.db, ctx.user_id, xp::skill_xp("writing").unwrap(), "writing_completed", Some(&format!("attempt:{attempt_id}")), Some("writing")).await?;
        streak::record_activity(&state.db, ctx.user_id, chrono::Utc::now()).await?;
        achievement::check_and_award(&state.db, ctx.user_id, &achievement::ActivityContext { skill_category: Some("writing".to_string()), question_ids: vec![] }).await?;
        daily_mission::record_progress(&state.db, ctx.user_id, Some("writing"), chrono::Utc::now()).await?;

        return Ok(Json(serde_json::to_value(result).map_err(|e| AppError::Internal(e.into()))?));
    }

    let answers = body.answers.unwrap_or_default();
    let result = assessment::submit_attempt(&state.db, &state.config, &ctx, attempt_id, &answers).await?;

    let assessment_id = attempt.assessment_id.ok_or_else(|| AppError::Internal(anyhow::anyhow!("attempt has no assessment_id")))?;
    let assessment_type = assessment::find_assessment_type(&state.db, assessment_id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("attempt references a missing assessment")))?;

    xp::award_xp(&state.db, ctx.user_id, assessment::assessment_xp(&assessment_type), "assessment_completed", Some(&format!("attempt:{attempt_id}")), None).await?;
    streak::record_activity(&state.db, ctx.user_id, chrono::Utc::now()).await?;
    achievement::check_and_award(&state.db, ctx.user_id, &achievement::ActivityContext { skill_category: None, question_ids: answers.keys().copied().collect() }).await?;
    daily_mission::record_progress(&state.db, ctx.user_id, None, chrono::Utc::now()).await?;
    exam_session::record_submission(&state.db, assessment_id, ctx.user_id, chrono::Utc::now()).await?;

    Ok(Json(serde_json::to_value(result).map_err(|e| AppError::Internal(e.into()))?))
}
