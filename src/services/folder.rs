use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::asset;
use crate::services::drive_permissions::{require_access, require_owner, DriveResource, Permission};
use crate::services::resource_activity::log_activity;

// Port of folder_repository.ts + the folder-related half of
// drive_service.ts. Adjacency-list tree via parent_folder_id (self-FK),
// WITH RECURSIVE ancestor/descendant walks mirror concept_repository's
// existing tree pattern (ADR-0007) rather than a new tree-storage scheme.

#[derive(Debug, Clone, serde::Serialize)]
pub struct Folder {
    pub id: Uuid,
    pub name: String,
    pub parent_folder_id: Option<Uuid>,
    pub owner_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip)]
    pub deleted_at: Option<DateTime<Utc>>,
}

async fn insert(pool: &PgPool, name: &str, parent_folder_id: Option<Uuid>, owner_id: Uuid) -> Result<Folder, AppError> {
    let row = sqlx::query_as!(
        Folder,
        r#"insert into folders (name, parent_folder_id, owner_id) values ($1, $2, $3)
           returning id, name, parent_folder_id, owner_id, created_at, updated_at, deleted_at"#,
        name,
        parent_folder_id,
        owner_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Folder>, AppError> {
    let row = sqlx::query_as!(Folder, r#"select id, name, parent_folder_id, owner_id, created_at, updated_at, deleted_at from folders where id = $1"#, id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

// Every ancestor of `id`, root-first, excluding `id` itself.
pub async fn find_ancestor_ids(pool: &PgPool, id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"with recursive ancestors as (
             select f.id, f.parent_folder_id, 0 as depth from folders f where f.id = $1
             union all
             select f.id, f.parent_folder_id, a.depth + 1 from folders f join ancestors a on f.id = a.parent_folder_id
           )
           select id as "id!" from ancestors where id != $1 order by depth desc"#,
        id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// Every descendant folder of `id` (any depth), excluding `id` itself.
pub async fn find_descendant_ids(pool: &PgPool, id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"with recursive descendants as (
             select f.id from folders f where f.parent_folder_id = $1
             union all
             select f.id from folders f join descendants d on f.parent_folder_id = d.id
           )
           select id as "id!" from descendants"#,
        id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub struct FolderPage {
    pub items: Vec<Folder>,
    pub next_cursor: Option<Uuid>,
}

// Keyset-paginated by id. parent_folder_id = None lists root-level folders.
async fn list_children(pool: &PgPool, parent_folder_id: Option<Uuid>, cursor: Option<Uuid>, limit: i64) -> Result<FolderPage, AppError> {
    let mut rows = sqlx::query_as!(
        Folder,
        r#"select id, name, parent_folder_id, owner_id, created_at, updated_at, deleted_at from folders
           where deleted_at is null
             and parent_folder_id is not distinct from $1
             and ($2::uuid is null or id > $2)
           order by id asc
           limit $3"#,
        parent_folder_id,
        cursor,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;
    let next_cursor = if rows.len() > limit as usize { rows.pop().map(|r| r.id) } else { None };
    Ok(FolderPage { items: rows, next_cursor })
}

async fn rename(pool: &PgPool, id: Uuid, name: &str) -> Result<Folder, AppError> {
    let row = sqlx::query_as!(
        Folder,
        r#"update folders set name = $2, updated_at = now() where id = $1
           returning id, name, parent_folder_id, owner_id, created_at, updated_at, deleted_at"#,
        id,
        name,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn move_to(pool: &PgPool, id: Uuid, new_parent_folder_id: Option<Uuid>) -> Result<Folder, AppError> {
    let row = sqlx::query_as!(
        Folder,
        r#"update folders set parent_folder_id = $2, updated_at = now() where id = $1
           returning id, name, parent_folder_id, owner_id, created_at, updated_at, deleted_at"#,
        id,
        new_parent_folder_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn soft_delete(pool: &PgPool, ids: &[Uuid]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query!(r#"update folders set deleted_at = now() where id = any($1)"#, ids).execute(pool).await?;
    Ok(())
}

async fn restore(pool: &PgPool, id: Uuid) -> Result<Folder, AppError> {
    let row = sqlx::query_as!(
        Folder,
        r#"update folders set deleted_at = null, updated_at = now() where id = $1
           returning id, name, parent_folder_id, owner_id, created_at, updated_at, deleted_at"#,
        id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn permanent_delete(pool: &PgPool, ids: &[Uuid]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query!(r#"delete from folders where id = any($1)"#, ids).execute(pool).await?;
    Ok(())
}

async fn list_trash(pool: &PgPool, owner_id: Uuid, cursor: Option<Uuid>, limit: i64) -> Result<FolderPage, AppError> {
    let mut rows = sqlx::query_as!(
        Folder,
        r#"select id, name, parent_folder_id, owner_id, created_at, updated_at, deleted_at from folders
           where owner_id = $1 and deleted_at is not null and ($2::uuid is null or id > $2)
           order by id asc
           limit $3"#,
        owner_id,
        cursor,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;
    let next_cursor = if rows.len() > limit as usize { rows.pop().map(|r| r.id) } else { None };
    Ok(FolderPage { items: rows, next_cursor })
}

// --- Service layer (drive_service.ts's folder-related half) ---

pub async fn create_folder(pool: &PgPool, ctx: &AuthContext, name: &str, parent_folder_id: Option<Uuid>) -> Result<Folder, AppError> {
    if let Some(parent_id) = parent_folder_id {
        require_access(pool, ctx, DriveResource::Folder, parent_id, Permission::Editor).await?;
    }
    let folder = insert(pool, name, parent_folder_id, ctx.user_id).await?;
    log_activity(pool, DriveResource::Folder, folder.id, ctx.user_id, "create", None).await;
    Ok(folder)
}

pub async fn rename_folder(pool: &PgPool, ctx: &AuthContext, id: Uuid, name: &str) -> Result<Folder, AppError> {
    require_access(pool, ctx, DriveResource::Folder, id, Permission::Editor).await?;
    let folder = rename(pool, id, name).await?;
    log_activity(pool, DriveResource::Folder, id, ctx.user_id, "rename", Some(serde_json::json!({"new_name": name}))).await;
    Ok(folder)
}

pub async fn move_folder(pool: &PgPool, ctx: &AuthContext, id: Uuid, new_parent_folder_id: Option<Uuid>) -> Result<Folder, AppError> {
    require_access(pool, ctx, DriveResource::Folder, id, Permission::Editor).await?;
    if let Some(new_parent_id) = new_parent_folder_id {
        if new_parent_id == id {
            return Err(AppError::UnprocessableEntity("invalid_move", "a folder cannot be moved into itself".to_string()));
        }
        require_access(pool, ctx, DriveResource::Folder, new_parent_id, Permission::Editor).await?;
        let descendants = find_descendant_ids(pool, id).await?;
        if descendants.contains(&new_parent_id) {
            return Err(AppError::UnprocessableEntity("invalid_move", "a folder cannot be moved into its own descendant".to_string()));
        }
    }
    let folder = move_to(pool, id, new_parent_folder_id).await?;
    log_activity(pool, DriveResource::Folder, id, ctx.user_id, "move", None).await;
    Ok(folder)
}

// Cascades to every descendant folder and every asset inside any of them.
pub async fn delete_folder(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<(), AppError> {
    require_access(pool, ctx, DriveResource::Folder, id, Permission::Editor).await?;
    let mut ids = find_descendant_ids(pool, id).await?;
    ids.push(id);
    soft_delete(pool, &ids).await?;
    asset::soft_delete_in_folders(pool, &ids).await?;
    log_activity(pool, DriveResource::Folder, id, ctx.user_id, "delete", None).await;
    Ok(())
}

// Does NOT cascade-restore descendants or assets swept into trash by an
// earlier cascade delete — only the single folder row.
pub async fn restore_folder(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<Folder, AppError> {
    require_owner(pool, ctx, DriveResource::Folder, id).await?;
    let folder = restore(pool, id).await?;
    log_activity(pool, DriveResource::Folder, id, ctx.user_id, "restore", None).await;
    Ok(folder)
}

// No activity log call at all for this path (unlike every other
// mutation) — matches the Bun original exactly.
pub async fn permanent_delete_folder(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<(), AppError> {
    require_owner(pool, ctx, DriveResource::Folder, id).await?;
    let mut ids = find_descendant_ids(pool, id).await?;
    ids.push(id);
    // safety: never orphan an asset with a dangling folder_id
    asset::soft_delete_in_folders(pool, &ids).await?;
    permanent_delete(pool, &ids).await?;
    Ok(())
}

// --- Listing (used by services::drive) ---

pub async fn children_page(pool: &PgPool, parent_folder_id: Option<Uuid>, cursor: Option<Uuid>, limit: i64) -> Result<FolderPage, AppError> {
    list_children(pool, parent_folder_id, cursor, limit).await
}

pub async fn trash_page(pool: &PgPool, owner_id: Uuid, cursor: Option<Uuid>, limit: i64) -> Result<FolderPage, AppError> {
    list_trash(pool, owner_id, cursor, limit).await
}
