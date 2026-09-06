use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::learning::QueueQuery;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::{frss, learning_queue, mastery, rescue};
use crate::state::AppState;

// GET /mastery/{concept_id}
pub async fn get_mastery(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(concept_id): Path<Uuid>,
) -> Result<Json<mastery::MasteryResponse>, AppError> {
    require_permission(&ctx, Resource::Mastery, Action::View)?;
    Ok(Json(mastery::get_mastery(&state.db, &state.config, ctx.user_id, concept_id).await?))
}

// GET /concepts/{id}/mastery-breakdown
pub async fn get_mastery_breakdown(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(concept_id): Path<Uuid>,
) -> Result<Json<mastery::MasteryBreakdownNode>, AppError> {
    require_permission(&ctx, Resource::Mastery, Action::View)?;
    Ok(Json(mastery::get_mastery_breakdown(&state.db, &state.config, ctx.user_id, concept_id).await?))
}

// GET /concepts/{id}/rescue-status
pub async fn get_rescue_status(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(concept_id): Path<Uuid>,
) -> Result<Json<rescue::RescueStatusResponse>, AppError> {
    require_permission(&ctx, Resource::Mastery, Action::View)?;
    Ok(Json(rescue::get_rescue_status(&state.db, &state.config, ctx.user_id, concept_id).await?))
}

// GET /review-queue?limit=
pub async fn get_review_queue(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Query(query): Query<QueueQuery>,
) -> Result<Json<frss::ReviewQueueResponse>, AppError> {
    require_permission(&ctx, Resource::Mastery, Action::View)?;
    Ok(Json(frss::get_review_queue(&state.db, &state.config, ctx.user_id, query.limit).await?))
}

// GET /learning-queue?limit=
pub async fn get_learning_queue(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Query(query): Query<QueueQuery>,
) -> Result<Json<learning_queue::LearningQueueResponse>, AppError> {
    require_permission(&ctx, Resource::Mastery, Action::View)?;
    Ok(Json(learning_queue::get_learning_queue(&state.db, &state.config, ctx.user_id, query.limit).await?))
}
