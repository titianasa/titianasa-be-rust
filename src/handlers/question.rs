use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::question::{CheckAnswerRequest, CreateQuestionBankRequest, CreateQuestionRequest, ListQuestionsQuery};
use crate::models::responses::question::{
    CheckAnswerResponse, ListQuestionBanksResponse, ListQuestionsResponse, QuestionBankResponse, QuestionDetailResponse,
    QuestionStatusResponse, QuestionStemResponse,
};
use crate::services::{achievement, daily_mission, question, streak, xp};
use crate::state::AppState;

// GET /question-banks
pub async fn get_question_banks(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
) -> Result<Json<ListQuestionBanksResponse>, AppError> {
    let items = question::list_banks(&state.db).await?;
    Ok(Json(ListQuestionBanksResponse { items }))
}

// POST /question-banks
pub async fn post_question_bank(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<CreateQuestionBankRequest>,
) -> Result<(StatusCode, Json<QuestionBankResponse>), AppError> {
    let result = question::create_bank(&state.db, &ctx, body.subject_id, &body.name).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /question-banks/{id}/questions
pub async fn post_question(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(bank_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateQuestionRequest>,
) -> Result<(StatusCode, Json<QuestionStatusResponse>), AppError> {
    let new_question = question::NewQuestion {
        bank_id,
        r#type: body.r#type,
        difficulty: body.difficulty,
        data: body.data,
        correct_answer: body.correct_answer,
        explanation: body.explanation,
        concept_ids: body.concept_ids.unwrap_or_default(),
        skill_category: body.skill_category,
        generated_by: "human".to_string(),
    };
    let result = question::create_question(&state.db, &ctx, new_question).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /question-banks/{id}/questions
pub async fn get_questions(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
    Path(bank_id): Path<Uuid>,
    Query(query): Query<ListQuestionsQuery>,
) -> Result<Json<ListQuestionsResponse>, AppError> {
    let result = question::list_by_bank(&state.db, bank_id, query.status, query.cursor, query.limit).await?;
    Ok(Json(result))
}

// GET /questions/{id}
pub async fn get_question(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(question_id): Path<Uuid>,
) -> Result<Json<QuestionDetailResponse>, AppError> {
    Ok(Json(question::get_question(&state.db, &ctx, question_id).await?))
}

// GET /questions/{id}/stem
pub async fn get_question_stem(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(question_id): Path<Uuid>,
) -> Result<Json<QuestionStemResponse>, AppError> {
    Ok(Json(question::get_question_stem(&state.db, &ctx, question_id).await?))
}

// POST /questions/{id}/submit-review
pub async fn post_submit_review(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(question_id): Path<Uuid>,
) -> Result<Json<QuestionStatusResponse>, AppError> {
    Ok(Json(question::submit_for_review(&state.db, &ctx, question_id).await?))
}

// POST /questions/{id}/publish
pub async fn post_publish(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(question_id): Path<Uuid>,
) -> Result<Json<QuestionStatusResponse>, AppError> {
    Ok(Json(question::publish(&state.db, &ctx, question_id).await?))
}

// POST /questions/{id}/reject
pub async fn post_reject(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(question_id): Path<Uuid>,
) -> Result<Json<QuestionStatusResponse>, AppError> {
    Ok(Json(question::reject(&state.db, &ctx, question_id).await?))
}

// POST /questions/{id}/check — P3-001. Drives the same XP/streak/
// achievement/daily-mission hook sequence as assessment's post_submit,
// composed here at the handler (not inside question::check_answer),
// matching question_handler.ts's postCheck exactly.
pub async fn post_check(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(question_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CheckAnswerRequest>,
) -> Result<Json<CheckAnswerResponse>, AppError> {
    let result = question::check_answer(&state.db, &state.config, &ctx, question_id, &body.submitted_answer).await?;

    let xp_amount = result.skill_category.as_deref().and_then(xp::skill_xp).unwrap_or(xp::UNTAGGED_QUESTION_XP);
    xp::award_xp(&state.db, ctx.user_id, xp_amount, "question_answered", None, result.skill_category.as_deref()).await?;
    streak::record_activity(&state.db, ctx.user_id, chrono::Utc::now()).await?;
    achievement::check_and_award(
        &state.db,
        ctx.user_id,
        &achievement::ActivityContext { skill_category: result.skill_category.clone(), question_ids: vec![question_id] },
    )
    .await?;
    daily_mission::record_progress(&state.db, ctx.user_id, result.skill_category.as_deref(), chrono::Utc::now()).await?;

    Ok(Json(CheckAnswerResponse {
        correct: result.correct,
        correct_answer: result.correct_answer,
        explanation: result.explanation,
        rescue_triggered: result.rescue_triggered,
        rescue_suggested_item_ids: result.rescue_suggested_item_ids,
    }))
}
