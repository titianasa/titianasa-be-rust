use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::drive_permissions::{require_access, require_owner, DriveResource, Permission};
use crate::services::resource_activity::log_activity;
use crate::services::storage::AssetStorage;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Asset {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub url: String,
    pub r#type: String,
    pub visibility: String,
    pub created_at: DateTime<Utc>,
    pub filename: Option<String>,
    pub folder_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip)]
    pub deleted_at: Option<DateTime<Utc>>,
}

// `id` is passed in (not DB-generated) because the caller already used
// it as the storage key before this insert.
#[allow(clippy::too_many_arguments)]
async fn insert(pool: &PgPool, id: Uuid, user_id: Uuid, url: &str, r#type: &str, visibility: &str, filename: Option<&str>, folder_id: Option<Uuid>) -> Result<Asset, AppError> {
    let row = sqlx::query_as!(
        Asset,
        r#"insert into assets (id, user_id, url, type, visibility, filename, folder_id)
           values ($1, $2, $3, $4, $5, $6, $7)
           returning id, user_id, url, type, visibility, created_at, filename, folder_id, updated_at, deleted_at"#,
        id,
        user_id,
        url,
        r#type,
        visibility,
        filename,
        folder_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Asset>, AppError> {
    let row = sqlx::query_as!(Asset, r#"select id, user_id, url, type, visibility, created_at, filename, folder_id, updated_at, deleted_at from assets where id = $1"#, id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub struct AssetPage {
    pub items: Vec<Asset>,
    pub next_cursor: Option<Uuid>,
}

// Keyset-paginated by id. folder_id = None lists root-level assets.
// type_prefix filters by MIME prefix for the media picker's tabs.
async fn list_in_folder(pool: &PgPool, folder_id: Option<Uuid>, type_prefix: Option<&str>, cursor: Option<Uuid>, limit: i64) -> Result<AssetPage, AppError> {
    let type_pattern = type_prefix.map(|p| format!("{p}%"));
    let mut rows = sqlx::query_as!(
        Asset,
        r#"select id, user_id, url, type, visibility, created_at, filename, folder_id, updated_at, deleted_at from assets
           where deleted_at is null
             and folder_id is not distinct from $1
             and ($2::text is null or type like $2)
             and ($3::uuid is null or id > $3)
           order by id asc
           limit $4"#,
        folder_id,
        type_pattern,
        cursor,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;
    let next_cursor = if rows.len() > limit as usize { rows.pop().map(|r| r.id) } else { None };
    Ok(AssetPage { items: rows, next_cursor })
}

async fn rename(pool: &PgPool, id: Uuid, filename: &str) -> Result<Asset, AppError> {
    let row = sqlx::query_as!(
        Asset,
        r#"update assets set filename = $2, updated_at = now() where id = $1
           returning id, user_id, url, type, visibility, created_at, filename, folder_id, updated_at, deleted_at"#,
        id,
        filename,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn move_to(pool: &PgPool, id: Uuid, folder_id: Option<Uuid>) -> Result<Asset, AppError> {
    let row = sqlx::query_as!(
        Asset,
        r#"update assets set folder_id = $2, updated_at = now() where id = $1
           returning id, user_id, url, type, visibility, created_at, filename, folder_id, updated_at, deleted_at"#,
        id,
        folder_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn soft_delete_in_folders(pool: &PgPool, folder_ids: &[Uuid]) -> Result<(), AppError> {
    if folder_ids.is_empty() {
        return Ok(());
    }
    sqlx::query!(r#"update assets set deleted_at = now() where folder_id = any($1)"#, folder_ids).execute(pool).await?;
    Ok(())
}

async fn soft_delete(pool: &PgPool, id: Uuid) -> Result<(), AppError> {
    sqlx::query!(r#"update assets set deleted_at = now() where id = $1"#, id).execute(pool).await?;
    Ok(())
}

async fn restore(pool: &PgPool, id: Uuid) -> Result<Asset, AppError> {
    let row = sqlx::query_as!(
        Asset,
        r#"update assets set deleted_at = null, updated_at = now() where id = $1
           returning id, user_id, url, type, visibility, created_at, filename, folder_id, updated_at, deleted_at"#,
        id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// Hard-deletes the DB row only — the R2 object is NEVER removed
// (AssetStorage has no delete method at all), reproduced as-is: R2
// objects are permanently orphaned on hard-delete.
async fn permanent_delete(pool: &PgPool, id: Uuid) -> Result<(), AppError> {
    sqlx::query!(r#"delete from assets where id = $1"#, id).execute(pool).await?;
    Ok(())
}

async fn list_trash(pool: &PgPool, owner_id: Uuid, cursor: Option<Uuid>, limit: i64) -> Result<AssetPage, AppError> {
    let mut rows = sqlx::query_as!(
        Asset,
        r#"select id, user_id, url, type, visibility, created_at, filename, folder_id, updated_at, deleted_at from assets
           where user_id = $1 and deleted_at is not null and ($2::uuid is null or id > $2)
           order by id asc
           limit $3"#,
        owner_id,
        cursor,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;
    let next_cursor = if rows.len() > limit as usize { rows.pop().map(|r| r.id) } else { None };
    Ok(AssetPage { items: rows, next_cursor })
}

// --- Upload flows (asset_service.ts) ---

pub struct UploadedAsset {
    pub id: Uuid,
    pub url: String,
    pub r#type: String,
    pub filename: Option<String>,
}

// POST /assets, /assets/upload — proxy upload: the caller already
// checked config.asset_max_bytes before calling this. folder_id/filename
// are the Drive feature's additions — None/root for callers that don't
// care.
pub async fn upload(pool: &PgPool, storage: &dyn AssetStorage, signed_url_ttl_seconds: u64, user_id: Uuid, bytes: Vec<u8>, content_type: &str, filename: Option<&str>, folder_id: Option<Uuid>) -> Result<UploadedAsset, AppError> {
    let asset_id = Uuid::new_v4();
    let key = asset_id.to_string();

    storage.put(&key, bytes, content_type).await?;
    let url = storage.signed_url(&key, signed_url_ttl_seconds).await?;

    // P1-010's proxy upload predates the public/private distinction —
    // defaults to "private", unchanged behavior for existing callers.
    let asset = insert(pool, asset_id, user_id, &url, content_type, "private", filename, folder_id).await?;
    log_activity(pool, DriveResource::Asset, asset.id, user_id, "upload", None).await;

    Ok(UploadedAsset { id: asset.id, url: asset.url, r#type: asset.r#type, filename: asset.filename })
}

pub struct PresignedUpload {
    pub asset_id: Uuid,
    pub upload_url: String,
}

// POST /assets/presigned-upload — mints a fresh, server-chosen key and
// hands the caller a presigned PUT URL for it; the client uploads bytes
// directly to R2. No DB row is created here.
pub async fn presigned_upload(storage: &dyn AssetStorage, presigned_put_ttl_seconds: u64, content_type: &str) -> Result<PresignedUpload, AppError> {
    let asset_id = Uuid::new_v4();
    let key = asset_id.to_string();
    let upload_url = storage.presigned_put_url(&key, content_type, presigned_put_ttl_seconds).await?;
    Ok(PresignedUpload { asset_id, upload_url })
}

// POST /assets/confirm — verifies the object claimed by asset_id
// actually exists in R2 before trusting the client's "I uploaded it"
// claim and writing the assets row.
#[allow(clippy::too_many_arguments)]
pub async fn confirm_upload(
    pool: &PgPool,
    storage: &dyn AssetStorage,
    signed_url_ttl_seconds: u64,
    public_signed_url_ttl_seconds: u64,
    user_id: Uuid,
    asset_id: Uuid,
    content_type: &str,
    visibility: &str,
    filename: Option<&str>,
    folder_id: Option<Uuid>,
) -> Result<UploadedAsset, AppError> {
    if visibility != "public" && visibility != "private" {
        return Err(AppError::UnprocessableEntity("invalid_visibility", "visibility must be \"public\" or \"private\"".to_string()));
    }

    let key = asset_id.to_string();
    if !storage.exists(&key).await? {
        return Err(AppError::UnprocessableEntity("asset_not_uploaded", "no object found for this asset_id — upload the file to the presigned URL before confirming".to_string()));
    }

    let ttl = if visibility == "public" { public_signed_url_ttl_seconds } else { signed_url_ttl_seconds };
    let url = storage.signed_url(&key, ttl).await?;

    let asset = insert(pool, asset_id, user_id, &url, content_type, visibility, filename, folder_id).await?;
    log_activity(pool, DriveResource::Asset, asset.id, user_id, "upload", None).await;

    Ok(UploadedAsset { id: asset.id, url: asset.url, r#type: asset.r#type, filename: asset.filename })
}

// --- Drive mutations (drive_service.ts's asset-related half) ---

pub async fn rename_asset(pool: &PgPool, ctx: &AuthContext, id: Uuid, filename: &str) -> Result<Asset, AppError> {
    require_access(pool, ctx, DriveResource::Asset, id, Permission::Editor).await?;
    let asset = rename(pool, id, filename).await?;
    log_activity(pool, DriveResource::Asset, id, ctx.user_id, "rename", Some(serde_json::json!({"new_name": filename}))).await;
    Ok(asset)
}

pub async fn move_asset(pool: &PgPool, ctx: &AuthContext, id: Uuid, folder_id: Option<Uuid>) -> Result<Asset, AppError> {
    require_access(pool, ctx, DriveResource::Asset, id, Permission::Editor).await?;
    if let Some(folder_id) = folder_id {
        require_access(pool, ctx, DriveResource::Folder, folder_id, Permission::Editor).await?;
    }
    let asset = move_to(pool, id, folder_id).await?;
    log_activity(pool, DriveResource::Asset, id, ctx.user_id, "move", None).await;
    Ok(asset)
}

pub async fn delete_asset(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<(), AppError> {
    require_access(pool, ctx, DriveResource::Asset, id, Permission::Editor).await?;
    soft_delete(pool, id).await?;
    log_activity(pool, DriveResource::Asset, id, ctx.user_id, "delete", None).await;
    Ok(())
}

pub async fn restore_asset(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<Asset, AppError> {
    require_owner(pool, ctx, DriveResource::Asset, id).await?;
    let asset = restore(pool, id).await?;
    log_activity(pool, DriveResource::Asset, id, ctx.user_id, "restore", None).await;
    Ok(asset)
}

pub async fn permanent_delete_asset(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<(), AppError> {
    require_owner(pool, ctx, DriveResource::Asset, id).await?;
    permanent_delete(pool, id).await
}

// --- Listing (used by services::drive) ---

pub async fn in_folder_page(pool: &PgPool, folder_id: Option<Uuid>, type_prefix: Option<&str>, cursor: Option<Uuid>, limit: i64) -> Result<AssetPage, AppError> {
    list_in_folder(pool, folder_id, type_prefix, cursor, limit).await
}

pub async fn trash_page(pool: &PgPool, owner_id: Uuid, cursor: Option<Uuid>, limit: i64) -> Result<AssetPage, AppError> {
    list_trash(pool, owner_id, cursor, limit).await
}
