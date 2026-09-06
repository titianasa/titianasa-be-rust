use axum::{
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::drive::{ConfirmUploadRequest, ListAssetsQuery, MoveAssetRequest, PresignedUploadRequest, RenameAssetRequest, ShareRequest};
use crate::services::asset::{self, Asset};
use crate::services::drive_permissions::{resolve_access, DriveResource};
use crate::services::resource_activity;
use crate::services::resource_share;
use crate::services::storage::AssetStorage;
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct AssetDetailResponse {
    pub id: Uuid,
    pub url: String,
    pub r#type: String,
    pub filename: Option<String>,
    pub folder_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub visibility: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// The DB's stored `url` is never trusted directly — re-signed fresh on
// every read, since a signed URL's TTL may have already elapsed by the
// time it's read back.
async fn resign(storage: &dyn AssetStorage, config: &Config, asset: &Asset) -> Result<String, AppError> {
    let ttl = if asset.visibility == "public" { config.asset_public_signed_url_ttl_seconds } else { config.asset_signed_url_ttl_seconds };
    Ok(storage.signed_url(&asset.id.to_string(), ttl as u64).await?)
}

pub async fn to_detail(storage: &dyn AssetStorage, config: &Config, asset: Asset) -> Result<AssetDetailResponse, AppError> {
    let url = resign(storage, config, &asset).await?;
    Ok(AssetDetailResponse {
        id: asset.id,
        url,
        r#type: asset.r#type,
        filename: asset.filename,
        folder_id: asset.folder_id,
        owner_id: asset.user_id,
        visibility: asset.visibility,
        created_at: asset.created_at,
        updated_at: asset.updated_at,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct UploadAssetResponse {
    pub id: Uuid,
    pub url: String,
    pub r#type: String,
    pub filename: Option<String>,
}

// POST /assets, /assets/upload — expects a multipart field named "file";
// folder_id is an optional second field. Any authenticated user may
// upload (no RBAC row for "Asset: upload"). Axum's Multipart, unlike
// Elysia/Bun's, DOES stream field-by-field — but the oversized-upload
// check still happens after the "file" field is fully read into memory
// (needed to know its size), matching the Bun original's own documented
// "can't bail before buffering" limitation for a different reason.
pub async fn post_upload(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, mut multipart: Multipart) -> Result<(StatusCode, Json<UploadAssetResponse>), AppError> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut content_type: Option<String> = None;
    let mut filename: Option<String> = None;
    let mut folder_id: Option<Uuid> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| AppError::UnprocessableEntity("invalid_multipart", e.to_string()))? {
        match field.name().unwrap_or("") {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                content_type = field.content_type().map(|s| s.to_string());
                let bytes = field.bytes().await.map_err(|e| AppError::UnprocessableEntity("invalid_multipart", e.to_string()))?;
                file_bytes = Some(bytes.to_vec());
            }
            "folder_id" => {
                let text = field.text().await.map_err(|e| AppError::UnprocessableEntity("invalid_multipart", e.to_string()))?;
                folder_id = Uuid::parse_str(text.trim()).ok();
            }
            _ => {}
        }
    }

    let bytes = file_bytes.ok_or_else(|| AppError::UnprocessableEntity("file_required", "a \"file\" multipart field is required".to_string()))?;
    if bytes.len() as i64 > state.config.asset_max_bytes {
        return Err(AppError::PayloadTooLarge("file_too_large"));
    }
    let content_type = content_type.filter(|s| !s.is_empty()).unwrap_or_else(|| "application/octet-stream".to_string());

    let uploaded = asset::upload(&state.db, state.storage.as_ref(), state.config.asset_signed_url_ttl_seconds as u64, ctx.user_id, bytes, &content_type, filename.as_deref(), folder_id).await?;
    Ok((StatusCode::CREATED, Json(UploadAssetResponse { id: uploaded.id, url: uploaded.url, r#type: uploaded.r#type, filename: uploaded.filename })))
}

#[derive(Debug, serde::Serialize)]
pub struct PresignedUploadResponse {
    pub asset_id: Uuid,
    pub upload_url: String,
}

// POST /assets/presigned-upload
pub async fn post_presigned_upload(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<PresignedUploadRequest>,
) -> Result<(StatusCode, Json<PresignedUploadResponse>), AppError> {
    let result = asset::presigned_upload(state.storage.as_ref(), state.config.asset_presigned_put_ttl_seconds as u64, &body.content_type).await?;
    Ok((StatusCode::CREATED, Json(PresignedUploadResponse { asset_id: result.asset_id, upload_url: result.upload_url })))
}

// POST /assets/confirm
pub async fn post_confirm(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<ConfirmUploadRequest>) -> Result<(StatusCode, Json<UploadAssetResponse>), AppError> {
    let uploaded = asset::confirm_upload(
        &state.db,
        state.storage.as_ref(),
        state.config.asset_signed_url_ttl_seconds as u64,
        state.config.asset_public_signed_url_ttl_seconds as u64,
        ctx.user_id,
        body.asset_id,
        &body.content_type,
        &body.visibility,
        body.filename.as_deref(),
        body.folder_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(UploadAssetResponse { id: uploaded.id, url: uploaded.url, r#type: uploaded.r#type, filename: uploaded.filename })))
}

// GET /assets/{id}
pub async fn get_asset(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<AssetDetailResponse>, AppError> {
    let access = resolve_access(&state.db, &ctx, DriveResource::Asset, id).await?;
    if access.is_none() {
        return Err(AppError::NotFound("asset_not_found"));
    }
    let asset = asset::find_by_id(&state.db, id).await?.ok_or(AppError::NotFound("asset_not_found"))?;
    Ok(Json(to_detail(state.storage.as_ref(), &state.config, asset).await?))
}

#[derive(Debug, serde::Serialize)]
pub struct ListAssetsResponse {
    pub items: Vec<AssetDetailResponse>,
    pub next_cursor: Option<Uuid>,
}

// GET /assets?folder_id=&type=&cursor=&limit=
pub async fn get_assets(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<ListAssetsQuery>) -> Result<Json<ListAssetsResponse>, AppError> {
    if let Some(folder_id) = query.folder_id {
        let access = resolve_access(&state.db, &ctx, DriveResource::Folder, folder_id).await?;
        if access.is_none() {
            return Err(AppError::NotFound("folder_not_found"));
        }
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let page = asset::in_folder_page(&state.db, query.folder_id, query.r#type.as_deref(), query.cursor, limit).await?;
    let mut items = Vec::with_capacity(page.items.len());
    for a in page.items {
        items.push(to_detail(state.storage.as_ref(), &state.config, a).await?);
    }
    Ok(Json(ListAssetsResponse { items, next_cursor: page.next_cursor }))
}

// POST /assets/{id}/rename
pub async fn post_rename_asset(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<RenameAssetRequest>) -> Result<Json<AssetDetailResponse>, AppError> {
    let asset = asset::rename_asset(&state.db, &ctx, id, &body.filename).await?;
    Ok(Json(to_detail(state.storage.as_ref(), &state.config, asset).await?))
}

// POST /assets/{id}/move
pub async fn post_move_asset(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<MoveAssetRequest>) -> Result<Json<AssetDetailResponse>, AppError> {
    let asset = asset::move_asset(&state.db, &ctx, id, body.folder_id).await?;
    Ok(Json(to_detail(state.storage.as_ref(), &state.config, asset).await?))
}

// DELETE /assets/{id}
pub async fn delete_asset(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    asset::delete_asset(&state.db, &ctx, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// POST /assets/{id}/restore
pub async fn post_restore_asset(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<AssetDetailResponse>, AppError> {
    let asset = asset::restore_asset(&state.db, &ctx, id).await?;
    Ok(Json(to_detail(state.storage.as_ref(), &state.config, asset).await?))
}

// DELETE /assets/{id}/permanent
pub async fn delete_asset_permanent(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    asset::permanent_delete_asset(&state.db, &ctx, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, serde::Serialize)]
pub struct ShareResponse {
    pub id: Uuid,
    pub principal_type: String,
    pub principal_id: String,
    pub permission: String,
}

// GET /assets/{id}/shares
pub async fn get_asset_shares(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<Vec<ShareResponse>>, AppError> {
    let shares = resource_share::list_shares(&state.db, &ctx, DriveResource::Asset, id).await?;
    Ok(Json(shares.into_iter().map(|s| ShareResponse { id: s.id, principal_type: s.principal_type, principal_id: s.principal_id, permission: s.permission }).collect()))
}

// POST /assets/{id}/shares
pub async fn post_asset_share(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<ShareRequest>) -> Result<(StatusCode, Json<ShareResponse>), AppError> {
    let share = resource_share::share_resource(&state.db, &ctx, DriveResource::Asset, id, &body.principal_type, &body.principal_id, &body.permission).await?;
    Ok((StatusCode::CREATED, Json(ShareResponse { id: share.id, principal_type: share.principal_type, principal_id: share.principal_id, permission: share.permission })))
}

// DELETE /assets/{id}/shares/{share_id}
pub async fn delete_asset_share(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path((id, share_id)): Path<(Uuid, Uuid)>) -> Result<StatusCode, AppError> {
    resource_share::unshare_resource(&state.db, &ctx, DriveResource::Asset, id, share_id).await?;
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

// GET /assets/{id}/activity
pub async fn get_asset_activity(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<Vec<ActivityResponse>>, AppError> {
    let activity = resource_activity::list_activity(&state.db, &ctx, DriveResource::Asset, id).await?;
    Ok(Json(activity.into_iter().map(|a| ActivityResponse { id: a.id, actor_id: a.actor_id, action: a.action, detail: a.detail, created_at: a.created_at }).collect()))
}
