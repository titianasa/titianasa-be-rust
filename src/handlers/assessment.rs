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
use crate::models::requests::assessment::{CreateAssessmentRequest, GradeManualGroupRequest, SubmitAttemptRequest};
use crate::models::responses::assessment::AssessmentSummary;
use crate::services::assessment::{self, CreateAttemptResponse, CreateLessonAttemptResponse};
use crate::services::{achievement, daily_mission, exam_session, quiz_attempt, quiz_subtype, streak, xp};
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
        // Phase 37 — every item-anchored attempt is a `quiz` item now
        // (writing/speaking retired, see migrations/0038). Skill XP is
        // keyed off the FIRST question group's subtype family when it
        // maps to an existing xp::skill_xp tier (reading/listening/
        // grammar/vocabulary); production/interactive subjects don't
        // have an English-specific skill category, so those fall back
        // to the same flat tier assessment_xp already uses for an
        // untyped assessment.
        let quiz_config = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, item_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("attempt references a missing module item")))?;
        let skill_category = quiz_config
            .as_ref()
            .and_then(|c| c.get("question_groups"))
            .and_then(|g| g.as_array())
            .and_then(|groups| groups.first())
            .and_then(|g| g.get("subtype")).and_then(|v| v.as_str())
            .and_then(quiz_subtype::find)
            .and_then(|info| match info.family {
                quiz_subtype::SubtypeFamily::Reading => Some("reading"),
                quiz_subtype::SubtypeFamily::Listening => Some("listening"),
                quiz_subtype::SubtypeFamily::Grammar => Some("grammar"),
                quiz_subtype::SubtypeFamily::Vocabulary => Some("vocabulary"),
                quiz_subtype::SubtypeFamily::Production | quiz_subtype::SubtypeFamily::Interactive => None,
            });

        let quiz_answers = body.quiz_answers.unwrap_or_default();
        let result = quiz_attempt::submit_quiz_attempt(&state.db, &state.config, state.ai_provider.as_ref(), state.text_ai_provider.as_ref(), state.storage.as_ref(), &ctx, attempt_id, &quiz_answers).await?;

        let xp_amount = skill_category.and_then(xp::skill_xp).unwrap_or(20);
        xp::award_xp(&state.db, ctx.user_id, xp_amount, "quiz_completed", Some(&format!("attempt:{attempt_id}")), skill_category).await?;
        streak::record_activity(&state.db, ctx.user_id, chrono::Utc::now()).await?;
        achievement::check_and_award(&state.db, ctx.user_id, &achievement::ActivityContext { skill_category: skill_category.map(String::from), question_ids: vec![] }).await?;
        daily_mission::record_progress(&state.db, ctx.user_id, skill_category, chrono::Utc::now()).await?;

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

// POST /attempts/{id}/grade — a teacher scores one Manual-mode quiz
// question group.
pub async fn post_grade_attempt(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(attempt_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<GradeManualGroupRequest>,
) -> Result<StatusCode, AppError> {
    quiz_attempt::grade_manual_group(&state.db, &ctx, attempt_id, &body.group_id, &body.question_number, body.score, body.feedback).await?;
    Ok(StatusCode::NO_CONTENT)
}
