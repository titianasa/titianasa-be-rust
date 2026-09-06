use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::asset::{self, Asset};
use crate::services::drive_permissions::{require_access, DriveResource, Permission};
use crate::services::folder::{self, Folder};
use crate::services::organization;
use crate::services::resource_share;

// Port of the listing half of drive_service.ts (listDrive/listTrash/
// listSharedWithMe) plus drive_handler.ts's getShareCandidates.

pub struct DriveListing {
    pub folders: Vec<Folder>,
    pub assets: Vec<Asset>,
    pub breadcrumb: Vec<Folder>,
    pub next_cursor: Option<Uuid>,
}

// GET /drive?folder_id=&cursor=&limit= — folders+assets at one level,
// root when folder_id is omitted, plus a breadcrumb.
pub async fn list_drive(pool: &PgPool, ctx: &AuthContext, folder_id: Option<Uuid>, cursor: Option<Uuid>, limit: i64) -> Result<DriveListing, AppError> {
    let mut breadcrumb = Vec::new();
    if let Some(folder_id) = folder_id {
        require_access(pool, ctx, DriveResource::Folder, folder_id, Permission::Viewer).await?;
        let mut ancestors = folder::find_ancestor_ids(pool, folder_id).await?;
        ancestors.push(folder_id);
        for id in ancestors {
            if let Some(f) = folder::find_by_id(pool, id).await? {
                breadcrumb.push(f);
            }
        }
    }

    let folder_page = folder::children_page(pool, folder_id, cursor, limit).await?;
    let asset_page = asset::in_folder_page(pool, folder_id, None, cursor, limit).await?;

    Ok(DriveListing { folders: folder_page.items, assets: asset_page.items, breadcrumb, next_cursor: folder_page.next_cursor.or(asset_page.next_cursor) })
}

// GET /drive/trash — only the caller's own trash (implicit filter by
// owner id, not a permission check).
pub async fn list_trash(pool: &PgPool, ctx: &AuthContext, cursor: Option<Uuid>, limit: i64) -> Result<DriveListing, AppError> {
    let folder_page = folder::trash_page(pool, ctx.user_id, cursor, limit).await?;
    let asset_page = asset::trash_page(pool, ctx.user_id, cursor, limit).await?;
    Ok(DriveListing { folders: folder_page.items, assets: asset_page.items, breadcrumb: vec![], next_cursor: folder_page.next_cursor.or(asset_page.next_cursor) })
}

// Everything shared directly with this user or their role, at any
// folder depth — deliberately NOT folder-scoped browsing (a flat list of
// shared roots, not a nested tree walk from here). Unpaginated.
pub async fn list_shared_with_me(pool: &PgPool, ctx: &AuthContext) -> Result<DriveListing, AppError> {
    let folder_shares = resource_share::find_matching_for_principal(pool, "folder", ctx.user_id, ctx.role.as_deref()).await?;
    let asset_shares = resource_share::find_matching_for_principal(pool, "asset", ctx.user_id, ctx.role.as_deref()).await?;

    let mut folders = Vec::new();
    for share in &folder_shares {
        if let Some(f) = folder::find_by_id(pool, share.resource_id).await? {
            if f.deleted_at.is_none() {
                folders.push(f);
            }
        }
    }
    let mut assets = Vec::new();
    for share in &asset_shares {
        if let Some(a) = asset::find_by_id(pool, share.resource_id).await? {
            if a.deleted_at.is_none() {
                assets.push(a);
            }
        }
    }

    Ok(DriveListing { folders, assets, breadcrumb: vec![], next_cursor: None })
}

// GET /drive/share-candidates?query=&limit= — scoped to the caller's own
// active organization; empty (not an error) if they have none.
pub async fn share_candidates(pool: &PgPool, ctx: &AuthContext, query: Option<&str>, limit: i64) -> Result<Vec<organization::ShareCandidate>, AppError> {
    let Some(organization_id) = ctx.organization_id else { return Ok(vec![]) };
    organization::search_share_candidates(pool, organization_id, query, limit).await
}
