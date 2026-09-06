use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::Response,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::module::{
    AddModulePrerequisiteRequest, CreateModuleItemRequest, CreateModuleRequest, ReorderModuleItemsRequest, ReorderModulesRequest, UpdateModuleItemContentRequest,
    UpdateModuleItemMetaRequest, UpdateModuleRequest,
};
use crate::models::responses::module::{
    ListModulesResponse, ModuleAncestorsResponse, ModulePrerequisitesResponse, ModuleResponse, ModuleItemDetailResponse, ModuleItemStatusResponse, ModuleItemTreeResponse,
};
use crate::services::{module, module_item, module_completion};
use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct ParentIdQuery {
    pub parent_id: Option<Uuid>,
}

// GET /modules?parent_id=
pub async fn get_children(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Query(query): Query<ParentIdQuery>) -> Result<Json<ListModulesResponse>, AppError> {
    Ok(Json(module::list_children(&state.db, query.parent_id).await?))
}

// POST /modules
pub async fn post_module(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<CreateModuleRequest>) -> Result<(StatusCode, Json<ModuleResponse>), AppError> {
    let result = module::create(&state.db, &ctx, body).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /modules/{id}
pub async fn get_module(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleResponse>, AppError> {
    Ok(Json(module::get(&state.db, id).await?))
}

// PATCH /modules/{id}
pub async fn patch_module(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<UpdateModuleRequest>) -> Result<Json<ModuleResponse>, AppError> {
    Ok(Json(module::update(&state.db, &ctx, id, body).await?))
}

// GET /modules/{id}/ancestors
pub async fn get_ancestors(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleAncestorsResponse>, AppError> {
    Ok(Json(module::list_ancestors(&state.db, id).await?))
}

// POST /modules/reorder
pub async fn post_reorder_modules(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<ReorderModulesRequest>) -> Result<StatusCode, AppError> {
    module::reorder(&state.db, &ctx, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /modules/{id}/prerequisites
pub async fn get_prerequisites(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModulePrerequisitesResponse>, AppError> {
    Ok(Json(module::list_prerequisites(&state.db, id).await?))
}

// POST /modules/{id}/prerequisites
pub async fn post_prerequisite(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<AddModulePrerequisiteRequest>,
) -> Result<StatusCode, AppError> {
    module::add_prerequisite(&state.db, &ctx, id, body).await?;
    Ok(StatusCode::CREATED)
}

// DELETE /modules/{id}/prerequisites/{prerequisite_module_id}
pub async fn delete_prerequisite(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path((id, prerequisite_module_id)): Path<(Uuid, Uuid)>) -> Result<StatusCode, AppError> {
    module::remove_prerequisite(&state.db, &ctx, id, prerequisite_module_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /modules/{id}/items
pub async fn get_items(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemTreeResponse>, AppError> {
    Ok(Json(module_item::get_tree(&state.db, id).await?))
}

// POST /modules/{id}/items
pub async fn post_item(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateModuleItemRequest>,
) -> Result<(StatusCode, Json<ModuleItemStatusResponse>), AppError> {
    let result = module_item::create(&state.db, &ctx, id, body).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// PATCH /module-items/{id}
pub async fn patch_item(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<UpdateModuleItemMetaRequest>,
) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::update_meta(&state.db, &ctx, id, body).await?))
}

// POST /module-items/reorder
pub async fn post_reorder_items(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<ReorderModuleItemsRequest>) -> Result<StatusCode, AppError> {
    module_item::reorder(&state.db, &ctx, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /module-items/{id}
pub async fn get_item(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemDetailResponse>, AppError> {
    Ok(Json(module_item::get_detail(&state.db, &ctx, id).await?))
}

// PUT /module-items/{id}
pub async fn put_item(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<UpdateModuleItemContentRequest>,
) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::update_content(&state.db, &ctx, id, &body.content, &body.format).await?))
}

// POST /module-items/{id}/submit-review
pub async fn post_submit_review(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::submit_for_review(&state.db, &ctx, id).await?))
}

// POST /module-items/{id}/publish
pub async fn post_publish(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::publish(&state.db, &ctx, id).await?))
}

// POST /module-items/{id}/reject
pub async fn post_reject(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::reject(&state.db, &ctx, id).await?))
}

// GET /module-items/{id}/speaking-prompt-audio — returns raw audio
// bytes, not JSON, matching speaking_room.rs's post_tts precedent.
pub async fn get_speaking_prompt_audio(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Response, AppError> {
    let prompt = module_item::get_speaking_prompt_text(&state.db, &ctx, id).await?;
    let result = state
        .ai_provider
        .synthesize_speech(&prompt.text, &state.config.ai_tts_default_voice, &state.config.ai_tts_model)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(Response::builder().status(StatusCode::OK).header(header::CONTENT_TYPE, result.content_type).body(Body::from(result.bytes)).unwrap())
}

// GET /module-items/{id}/completion-status — §37 (ADR-0012 P21-002).
pub async fn get_completion_status(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<module_completion::ItemCompletionStatusResponse>, AppError> {
    Ok(Json(module_completion::get_completion_status(&state.db, &state.config, ctx.user_id, id).await?))
}

// POST /module-items/{id}/skip-completion
pub async fn post_skip_completion(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<module_completion::SkipCompletionResponse>, AppError> {
    Ok(Json(module_completion::skip_completion(&state.db, &state.config, ctx.user_id, id).await?))
}
