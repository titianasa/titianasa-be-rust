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
use crate::models::requests::program::{AttachModuleRequest, CreateProgramRequest, ReorderProgramModulesRequest, UpdateProgramRequest};
use crate::models::responses::program::{ListProgramsResponse, ProgramResponse, ProgramSummary, ProgramTree};
use crate::services::program;
use crate::state::AppState;

// GET /programs
pub async fn get_programs(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>) -> Result<Json<ListProgramsResponse>, AppError> {
    Ok(Json(program::list(&state.db).await?))
}

// POST /programs
pub async fn post_program(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<CreateProgramRequest>) -> Result<(StatusCode, Json<ProgramResponse>), AppError> {
    let result = program::create(&state.db, &ctx, body).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /programs/{id}
pub async fn get_program(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ProgramSummary>, AppError> {
    Ok(Json(program::get(&state.db, id).await?))
}

// PATCH /programs/{id}
pub async fn patch_program(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<UpdateProgramRequest>) -> Result<Json<ProgramSummary>, AppError> {
    Ok(Json(program::update(&state.db, &ctx, id, body).await?))
}

// GET /programs/{id}/tree
pub async fn get_tree(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ProgramTree>, AppError> {
    Ok(Json(program::get_tree(&state.db, id).await?))
}

// POST /programs/{id}/modules
pub async fn post_attach_module(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<AttachModuleRequest>,
) -> Result<StatusCode, AppError> {
    program::attach_module(&state.db, &ctx, id, body).await?;
    Ok(StatusCode::CREATED)
}

// DELETE /programs/{id}/modules/{module_id}
pub async fn delete_module(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path((id, module_id)): Path<(Uuid, Uuid)>) -> Result<StatusCode, AppError> {
    program::detach_module(&state.db, &ctx, id, module_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// POST /programs/{id}/modules/reorder
pub async fn post_reorder_modules(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<ReorderProgramModulesRequest>,
) -> Result<StatusCode, AppError> {
    program::reorder_modules(&state.db, &ctx, id, body).await?;
    Ok(StatusCode::NO_CONTENT)
}
