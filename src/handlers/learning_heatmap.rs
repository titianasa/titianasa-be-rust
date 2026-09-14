use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::learning_heatmap::{self, HeatmapQuery, HeatmapResponse};
use crate::state::AppState;

// GET /me/learning-heatmap
pub async fn get_my_heatmap(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<HeatmapQuery>) -> Result<Json<HeatmapResponse>, AppError> {
    Ok(Json(learning_heatmap::for_me(&state.db, &ctx, query).await?))
}

// GET /admin/learning/heatmap
pub async fn get_platform_heatmap(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<HeatmapQuery>) -> Result<Json<HeatmapResponse>, AppError> {
    Ok(Json(learning_heatmap::for_platform(&state.db, &ctx, query).await?))
}

// GET /classes/{id}/learning-heatmap
pub async fn get_class_heatmap(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(class_id): Path<Uuid>, Query(query): Query<HeatmapQuery>) -> Result<Json<HeatmapResponse>, AppError> {
    Ok(Json(learning_heatmap::for_class(&state.db, &ctx, class_id, query).await?))
}
