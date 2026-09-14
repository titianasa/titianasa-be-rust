use axum::{
    extract::{Path, State},
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::content_factory::admin::{self, CoverageRow, CreateRunRequest, CreateRunResponse, ExemplarRow, ReviewRequest, RunActionRequest, RunDetail, RunRow, SeedReport, StandardRow, TahapTree, TaskDetail, UpsertStandardRequest};
use crate::state::AppState;

// GET /admin/content-factory/coverage
pub async fn get_coverage(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Vec<CoverageRow>>, AppError> {
    Ok(Json(admin::coverage(&state.db, &ctx).await?))
}

// GET /admin/content-factory/tahap/{id}
pub async fn get_tahap_tree(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<TahapTree>, AppError> {
    Ok(Json(admin::tahap_tree(&state.db, &ctx, id).await?))
}

// GET /admin/content-factory/runs
pub async fn get_runs(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Vec<RunRow>>, AppError> {
    Ok(Json(admin::list_runs(&state.db, &ctx).await?))
}

// POST /admin/content-factory/runs
pub async fn post_run(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Json(body): Json<CreateRunRequest>) -> Result<Json<CreateRunResponse>, AppError> {
    Ok(Json(admin::create_run(&state.db, &ctx, body).await?))
}

// GET /admin/content-factory/runs/{id}
pub async fn get_run(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<RunDetail>, AppError> {
    Ok(Json(admin::get_run(&state.db, &ctx, id).await?))
}

// POST /admin/content-factory/runs/{id}/action
pub async fn post_run_action(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, Json(body): Json<RunActionRequest>) -> Result<Json<RunRow>, AppError> {
    Ok(Json(admin::run_action(&state.db, &ctx, id, body).await?))
}

// GET /admin/content-factory/review
pub async fn get_review_queue(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Vec<admin::ReviewQueueRow>>, AppError> {
    Ok(Json(admin::review_queue(&state.db, &ctx).await?))
}

// GET /admin/content-factory/tasks/{id}
pub async fn get_task(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<TaskDetail>, AppError> {
    Ok(Json(admin::get_task(&state.db, &ctx, id).await?))
}

// POST /admin/content-factory/tasks/{id}/review
pub async fn post_task_review(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, Json(body): Json<ReviewRequest>) -> Result<Json<TaskDetail>, AppError> {
    Ok(Json(admin::review_task(&state.db, &ctx, id, body).await?))
}

// GET /admin/content-factory/standards
pub async fn get_standards(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Vec<StandardRow>>, AppError> {
    Ok(Json(admin::list_standards(&state.db, &ctx).await?))
}

// PUT /admin/content-factory/standards/{tahap_id}
pub async fn put_standard(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(tahap_id): Path<Uuid>, Json(body): Json<UpsertStandardRequest>) -> Result<Json<StandardRow>, AppError> {
    Ok(Json(admin::upsert_standard(&state.db, &ctx, tahap_id, body).await?))
}

// GET /admin/content-factory/exemplars
pub async fn get_exemplars(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Vec<ExemplarRow>>, AppError> {
    Ok(Json(admin::list_exemplars(&state.db, &ctx).await?))
}

// POST /admin/content-factory/seed
pub async fn post_seed(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<SeedReport>, AppError> {
    Ok(Json(admin::seed_from_metadata(&state.db, &ctx).await?))
}
