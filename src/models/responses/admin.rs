use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct AuditLogRowResponse {
    pub id: Uuid,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<Uuid>,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub reason: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct AuditLogListResponse {
    pub items: Vec<AuditLogRowResponse>,
    /// Opaque — pass back as-is in `?cursor=`. Encodes `(created_at, id)`
    /// of the last row so a page boundary that lands mid-timestamp (two
    /// actions in the same millisecond) still can't skip or repeat a row.
    pub next_cursor: Option<String>,
}
