use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::drive_permissions::{require_access, require_owner, DriveResource, Permission};
use crate::services::resource_activity::log_activity;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResourceShare {
    pub id: Uuid,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub principal_type: String,
    pub principal_id: String,
    pub permission: String,
    pub granted_by: Uuid,
    pub created_at: DateTime<Utc>,
}

async fn insert(pool: &PgPool, resource_type: &str, resource_id: Uuid, principal_type: &str, principal_id: &str, permission: &str, granted_by: Uuid) -> Result<ResourceShare, AppError> {
    // Upsert on the unique (resource_type, resource_id, principal_type,
    // principal_id) 4-tuple — re-sharing the same resource with the same
    // principal updates the permission in place (existing row's id
    // preserved), never duplicates.
    let row = sqlx::query_as!(
        ResourceShare,
        r#"insert into resource_shares (resource_type, resource_id, principal_type, principal_id, permission, granted_by)
           values ($1, $2, $3, $4, $5, $6)
           on conflict (resource_type, resource_id, principal_type, principal_id) do update
             set permission = excluded.permission
           returning id, resource_type, resource_id, principal_type, principal_id, permission, granted_by, created_at"#,
        resource_type,
        resource_id,
        principal_type,
        principal_id,
        permission,
        granted_by,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn list_for_resource(pool: &PgPool, resource_type: &str, resource_id: Uuid) -> Result<Vec<ResourceShare>, AppError> {
    let rows = sqlx::query_as!(
        ResourceShare,
        r#"select id, resource_type, resource_id, principal_type, principal_id, permission, granted_by, created_at
           from resource_shares where resource_type = $1 and resource_id = $2
           order by created_at asc"#,
        resource_type,
        resource_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// Every grant of resource_type shared with this user or their role,
// regardless of which specific resource — backs "Shared with me".
pub async fn find_matching_for_principal(pool: &PgPool, resource_type: &str, user_id: Uuid, role: Option<&str>) -> Result<Vec<ResourceShare>, AppError> {
    let rows = sqlx::query_as!(
        ResourceShare,
        r#"select id, resource_type, resource_id, principal_type, principal_id, permission, granted_by, created_at
           from resource_shares
           where resource_type = $1
             and ((principal_type = 'user' and principal_id = $2)
                  or ($3::text is not null and principal_type = 'role' and principal_id = $3))"#,
        resource_type,
        user_id.to_string(),
        role,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn delete_share(pool: &PgPool, id: Uuid) -> Result<u64, AppError> {
    let result = sqlx::query!(r#"delete from resource_shares where id = $1"#, id).execute(pool).await?;
    Ok(result.rows_affected())
}

// The permission grants that apply to any of resource_ids (the resource
// itself plus its ancestor folders) for either the caller's user id or
// their role — drive_permissions::resolve_access takes the highest of
// whatever comes back.
pub async fn find_matching(pool: &PgPool, resource_type: &str, resource_ids: &[Uuid], user_id: Uuid, role: Option<&str>) -> Result<Vec<ResourceShare>, AppError> {
    if resource_ids.is_empty() {
        return Ok(vec![]);
    }
    let rows = sqlx::query_as!(
        ResourceShare,
        r#"select id, resource_type, resource_id, principal_type, principal_id, permission, granted_by, created_at
           from resource_shares
           where resource_type = $1
             and resource_id = any($2)
             and ((principal_type = 'user' and principal_id = $3)
                  or ($4::text is not null and principal_type = 'role' and principal_id = $4))"#,
        resource_type,
        resource_ids,
        user_id.to_string(),
        role,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn share_resource(pool: &PgPool, ctx: &AuthContext, resource: DriveResource, resource_id: Uuid, principal_type: &str, principal_id: &str, permission: &str) -> Result<ResourceShare, AppError> {
    if principal_type != "user" && principal_type != "role" {
        return Err(AppError::UnprocessableEntity("invalid_principal_type", "principal_type must be \"user\" or \"role\"".to_string()));
    }
    if permission != "viewer" && permission != "editor" {
        return Err(AppError::UnprocessableEntity("invalid_permission", "permission must be \"viewer\" or \"editor\"".to_string()));
    }
    require_owner(pool, ctx, resource, resource_id).await?;
    let share = insert(pool, resource.as_str(), resource_id, principal_type, principal_id, permission, ctx.user_id).await?;
    log_activity(pool, resource, resource_id, ctx.user_id, "share", Some(serde_json::json!({"principal_type": principal_type, "principal_id": principal_id, "permission": permission}))).await;
    Ok(share)
}

// Note (reproduced from Bun as-is): delete_share removes by share_id
// alone, with no filter tying it to `resource`/`resource_id` — the
// require_owner(resource, resource_id) check on the PATH's resource is
// what actually gates this, not a match between the share row and the
// path. An owner of resource A could in principle delete a share row
// that's really for resource B, as long as they know its share_id.
pub async fn unshare_resource(pool: &PgPool, ctx: &AuthContext, resource: DriveResource, resource_id: Uuid, share_id: Uuid) -> Result<(), AppError> {
    require_owner(pool, ctx, resource, resource_id).await?;
    let deleted = delete_share(pool, share_id).await?;
    if deleted == 0 {
        return Err(AppError::NotFound("share_not_found"));
    }
    log_activity(pool, resource, resource_id, ctx.user_id, "unshare", None).await;
    Ok(())
}

pub async fn list_shares(pool: &PgPool, ctx: &AuthContext, resource: DriveResource, resource_id: Uuid) -> Result<Vec<ResourceShare>, AppError> {
    require_access(pool, ctx, resource, resource_id, Permission::Viewer).await?;
    list_for_resource(pool, resource.as_str(), resource_id).await
}
