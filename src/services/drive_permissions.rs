use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::{asset, folder, resource_share};

// Port of drive_permissions.ts — Drive-style per-resource access
// control, genuinely different from permissions.rs's static role matrix:
// access here depends on *which specific file/folder* and who
// owns/was-granted access to it, not just the caller's role.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Viewer,
    Editor,
}

impl Permission {
    fn rank(self) -> u8 {
        match self {
            Permission::Viewer => 0,
            Permission::Editor => 1,
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "viewer" => Some(Permission::Viewer),
            "editor" => Some(Permission::Editor),
            _ => None,
        }
    }
}

fn higher_permission(a: Option<Permission>, b: Option<Permission>) -> Option<Permission> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => Some(if a.rank() >= b.rank() { a } else { b }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveResource {
    Asset,
    Folder,
}

impl DriveResource {
    pub fn as_str(self) -> &'static str {
        match self {
            DriveResource::Asset => "asset",
            DriveResource::Folder => "folder",
        }
    }
}

// `None` means no access at all (the resource doesn't exist, or exists
// but nothing grants this user any permission on it) — callers turn that
// into 404 (unknown) or 403 (exists, no access) as appropriate.
pub async fn resolve_access(pool: &PgPool, ctx: &AuthContext, resource: DriveResource, resource_id: Uuid) -> Result<Option<Permission>, AppError> {
    if ctx.role.as_deref() == Some("platform_admin") {
        return Ok(Some(Permission::Editor));
    }

    // Owner check + the folder_id to walk ancestors from (an asset's own
    // folder_id, or the folder's own id for a folder). asset.user_id is
    // nullable at the DB level — a null owner simply never matches
    // ctx.user_id below (falls through to the share-based checks), it
    // does NOT short-circuit to "no access" the way a missing row does.
    let (owner_id, walk_from_folder_id): (Option<Uuid>, Option<Uuid>) = match resource {
        DriveResource::Asset => {
            let Some(a) = asset::find_by_id(pool, resource_id).await? else { return Ok(None) };
            (a.user_id, a.folder_id)
        }
        DriveResource::Folder => {
            let Some(f) = folder::find_by_id(pool, resource_id).await? else { return Ok(None) };
            (Some(f.owner_id), Some(f.id))
        }
    };

    if owner_id == Some(ctx.user_id) {
        return Ok(Some(Permission::Editor));
    }

    // The set of resource ids a share could match: the resource itself
    // (as its own type) plus every ancestor folder (shares on folders
    // always use resource_type = "folder").
    let mut folder_scope_ids: Vec<Uuid> = Vec::new();
    if let Some(walk_from) = walk_from_folder_id {
        folder_scope_ids.push(walk_from);
        folder_scope_ids.extend(folder::find_ancestor_ids(pool, walk_from).await?);
    }

    let mut best: Option<Permission> = None;

    let direct_shares = resource_share::find_matching(pool, resource.as_str(), &[resource_id], ctx.user_id, ctx.role.as_deref()).await?;
    for share in &direct_shares {
        best = higher_permission(best, Permission::from_str(&share.permission));
    }

    if !folder_scope_ids.is_empty() {
        let folder_shares = resource_share::find_matching(pool, "folder", &folder_scope_ids, ctx.user_id, ctx.role.as_deref()).await?;
        for share in &folder_shares {
            best = higher_permission(best, Permission::from_str(&share.permission));
        }
    }

    Ok(best)
}

pub async fn require_access(pool: &PgPool, ctx: &AuthContext, resource: DriveResource, resource_id: Uuid, need: Permission) -> Result<(), AppError> {
    let access = resolve_access(pool, ctx, resource, resource_id).await?;
    let Some(access) = access else { return Err(AppError::NotFound("resource_not_found")) };
    if access.rank() < need.rank() {
        return Err(AppError::ForbiddenWithCode("insufficient_permission"));
    }
    Ok(())
}

// Sharing and trash management are owner/platform_admin-only — an
// Editor grant lets you edit content, not decide who else can see it or
// manage someone else's trash.
pub async fn require_owner(pool: &PgPool, ctx: &AuthContext, resource: DriveResource, resource_id: Uuid) -> Result<(), AppError> {
    if ctx.role.as_deref() == Some("platform_admin") {
        return Ok(());
    }
    // A null asset.user_id (nullable at the DB level) simply never
    // equals ctx.user_id -> falls through to owner_only, matching the
    // Bun original's `ownerId !== ctx.userId` on a null ownerId exactly
    // (not a 404 — the resource DOES exist, it's just ownerless).
    let owner_id: Option<Uuid> = match resource {
        DriveResource::Asset => asset::find_by_id(pool, resource_id).await?.ok_or(AppError::NotFound("resource_not_found"))?.user_id,
        DriveResource::Folder => Some(folder::find_by_id(pool, resource_id).await?.ok_or(AppError::NotFound("resource_not_found"))?.owner_id),
    };
    if owner_id != Some(ctx.user_id) {
        return Err(AppError::ForbiddenWithCode("owner_only"));
    }
    Ok(())
}
