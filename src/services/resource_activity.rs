use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::drive_permissions::{require_access, DriveResource, Permission};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResourceActivity {
    pub id: Uuid,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub actor_id: Uuid,
    pub action: String,
    pub detail: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

async fn insert(pool: &PgPool, resource_type: &str, resource_id: Uuid, actor_id: Uuid, action: &str, detail: Option<serde_json::Value>) -> Result<ResourceActivity, AppError> {
    let row = sqlx::query_as!(
        ResourceActivity,
        r#"insert into resource_activity (resource_type, resource_id, actor_id, action, detail)
           values ($1, $2, $3, $4, $5)
           returning id, resource_type, resource_id, actor_id, action, detail, created_at"#,
        resource_type,
        resource_id,
        actor_id,
        action,
        detail,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn list_for_resource(pool: &PgPool, resource_type: &str, resource_id: Uuid) -> Result<Vec<ResourceActivity>, AppError> {
    let rows = sqlx::query_as!(
        ResourceActivity,
        r#"select id, resource_type, resource_id, actor_id, action, detail, created_at
           from resource_activity where resource_type = $1 and resource_id = $2
           order by created_at desc"#,
        resource_type,
        resource_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// Best-effort / fire-and-forget — a logging failure never fails the
// parent request, matching drive_service.ts's `log()` helper exactly
// (it only console.error's and swallows the error).
pub async fn log_activity(pool: &PgPool, resource: DriveResource, resource_id: Uuid, actor_id: Uuid, action: &str, detail: Option<serde_json::Value>) {
    if let Err(e) = insert(pool, resource.as_str(), resource_id, actor_id, action, detail).await {
        tracing::error!(error = ?e, resource = resource.as_str(), resource_id = %resource_id, "failed to write resource_activity row");
    }
}

pub async fn list_activity(pool: &PgPool, ctx: &AuthContext, resource: DriveResource, resource_id: Uuid) -> Result<Vec<ResourceActivity>, AppError> {
    require_access(pool, ctx, resource, resource_id, Permission::Viewer).await?;
    list_for_resource(pool, resource.as_str(), resource_id).await
}
