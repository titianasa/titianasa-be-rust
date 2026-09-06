use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::drive::{CreateFolderRequest, MoveFolderRequest, RenameFolderRequest, ShareRequest};
use crate::services::drive_permissions::DriveResource;
use crate::services::folder::{self, Folder};
use crate::services::resource_activity;
use crate::services::resource_share;
use crate::state::AppState;

// Folder itself already serializes to the exact wire shape ({id, name,
// parent_folder_id, owner_id, created_at, updated_at} — deleted_at is
// #[serde(skip)]), so no separate FolderResponse wrapper is needed
// (unlike Asset, which needs its url re-signed before it's wire-safe).

// POST /folders
pub async fn post_folder(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<CreateFolderRequest>) -> Result<(StatusCode, Json<Folder>), AppError> {
    let folder = folder::create_folder(&state.db, &ctx, &body.name, body.parent_folder_id).await?;
    Ok((StatusCode::CREATED, Json(folder)))
}

// POST /folders/{id}/rename
pub async fn post_rename_folder(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<RenameFolderRequest>) -> Result<Json<Folder>, AppError> {
    Ok(Json(folder::rename_folder(&state.db, &ctx, id, &body.name).await?))
}

// POST /folders/{id}/move
pub async fn post_move_folder(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<MoveFolderRequest>) -> Result<Json<Folder>, AppError> {
    Ok(Json(folder::move_folder(&state.db, &ctx, id, body.parent_folder_id).await?))
}

// DELETE /folders/{id}
pub async fn delete_folder(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    folder::delete_folder(&state.db, &ctx, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// POST /folders/{id}/restore
pub async fn post_restore_folder(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<Folder>, AppError> {
    Ok(Json(folder::restore_folder(&state.db, &ctx, id).await?))
}

// DELETE /folders/{id}/permanent
pub async fn delete_folder_permanent(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    folder::permanent_delete_folder(&state.db, &ctx, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, serde::Serialize)]
pub struct ShareResponse {
    pub id: Uuid,
    pub principal_type: String,
    pub principal_id: String,
    pub permission: String,
}

// GET /folders/{id}/shares
pub async fn get_folder_shares(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<Vec<ShareResponse>>, AppError> {
    let shares = resource_share::list_shares(&state.db, &ctx, DriveResource::Folder, id).await?;
    Ok(Json(shares.into_iter().map(|s| ShareResponse { id: s.id, principal_type: s.principal_type, principal_id: s.principal_id, permission: s.permission }).collect()))
}

// POST /folders/{id}/shares
pub async fn post_folder_share(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<ShareRequest>) -> Result<(StatusCode, Json<ShareResponse>), AppError> {
    let share = resource_share::share_resource(&state.db, &ctx, DriveResource::Folder, id, &body.principal_type, &body.principal_id, &body.permission).await?;
    Ok((StatusCode::CREATED, Json(ShareResponse { id: share.id, principal_type: share.principal_type, principal_id: share.principal_id, permission: share.permission })))
}

// DELETE /folders/{id}/shares/{share_id}
pub async fn delete_folder_share(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path((id, share_id)): Path<(Uuid, Uuid)>) -> Result<StatusCode, AppError> {
    resource_share::unshare_resource(&state.db, &ctx, DriveResource::Folder, id, share_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, serde::Serialize)]
pub struct ActivityResponse {
    pub id: Uuid,
    pub actor_id: Uuid,
    pub action: String,
    pub detail: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

// GET /folders/{id}/activity
pub async fn get_folder_activity(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<Vec<ActivityResponse>>, AppError> {
    let activity = resource_activity::list_activity(&state.db, &ctx, DriveResource::Folder, id).await?;
    Ok(Json(activity.into_iter().map(|a| ActivityResponse { id: a.id, actor_id: a.actor_id, action: a.action, detail: a.detail, created_at: a.created_at }).collect()))
}
