// P40-001 (ADR-0014 §1) — the permanent record of every Admin Pusat
// write. Every `Action::Manage` call site in this crate must call
// `record` after the action succeeds — approving/rejecting an AI
// proposal, changing AI model settings, rolling back a content version,
// toggling a catalog entry, and so on, as those land in later Phase 40
// tickets.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::responses::admin::{AuditLogListResponse, AuditLogRowResponse};
use crate::services::permissions::{require_permission, Action, Resource};

/// `actor_id` is the acting admin — `None` only for a system-initiated
/// action (a future autonomous agent decision, ADR-0013 L3/L4), never
/// for a human request; every HTTP call site here has an `AuthContext`
/// to supply it.
pub async fn record(
    pool: &PgPool,
    actor_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Option<Uuid>,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
    reason: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into admin_audit_log (actor_id, action, target_type, target_id, before, after, reason)
           values ($1, $2, $3, $4, $5, $6, $7)"#,
        actor_id,
        action,
        target_type,
        target_id,
        before,
        after,
        reason,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Opaque cursor = `"{created_at_rfc3339}_{id}"` of the last row already
/// seen. Not a base64/obfuscated token — this is an internal admin
/// endpoint, nothing here is sensitive to expose in a query string, and
/// a plain, greppable cursor is easier to debug than an encoded one.
fn parse_cursor(cursor: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let (ts, id) = cursor.rsplit_once('_').ok_or_else(|| AppError::UnprocessableEntity("invalid_cursor", "cursor tidak valid".into()))?;
    let ts = DateTime::parse_from_rfc3339(ts).map_err(|_| AppError::UnprocessableEntity("invalid_cursor", "cursor tidak valid".into()))?.with_timezone(&Utc);
    let id = Uuid::parse_str(id).map_err(|_| AppError::UnprocessableEntity("invalid_cursor", "cursor tidak valid".into()))?;
    Ok((ts, id))
}

/// Newest-first, keyset-paginated on `(created_at, id)` — see `admin_audit_log_created_at_idx`.
pub async fn list(pool: &PgPool, ctx: &AuthContext, cursor: Option<String>, limit: Option<i64>) -> Result<AuditLogListResponse, AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::View)?;
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let before = cursor.as_deref().map(parse_cursor).transpose()?;
    let (before_ts, before_id) = before.map(|(ts, id)| (Some(ts), Some(id))).unwrap_or((None, None));

    let rows = sqlx::query!(
        r#"select l.id, l.actor_id, u.name as "actor_name?", l.action, l.target_type, l.target_id, l.before, l.after, l.reason, l.created_at
           from admin_audit_log l
           left join users u on u.id = l.actor_id
           where $1::timestamptz is null or (l.created_at, l.id) < ($1, $2::uuid)
           order by l.created_at desc, l.id desc
           limit $3"#,
        before_ts,
        before_id,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;

    let mut items: Vec<AuditLogRowResponse> = rows
        .into_iter()
        .map(|r| AuditLogRowResponse {
            id: r.id,
            actor_id: r.actor_id,
            actor_name: r.actor_name,
            action: r.action,
            target_type: r.target_type,
            target_id: r.target_id,
            before: r.before,
            after: r.after,
            reason: r.reason,
            created_at: r.created_at,
        })
        .collect();

    let next_cursor = if items.len() > limit as usize {
        items.pop();
        items.last().map(|r| format!("{}_{}", r.created_at.to_rfc3339(), r.id))
    } else {
        None
    };

    Ok(AuditLogListResponse { items, next_cursor })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_well_formed_cursor_round_trips() {
        let id = Uuid::new_v4();
        let ts = Utc::now();
        let cursor = format!("{}_{}", ts.to_rfc3339(), id);
        let (parsed_ts, parsed_id) = parse_cursor(&cursor).unwrap();
        assert_eq!(parsed_id, id);
        // rfc3339 round-trip may lose sub-nanosecond precision only if
        // the source ever had more than nanosecond resolution — Utc::now()
        // doesn't, so this must be exact.
        assert_eq!(parsed_ts.to_rfc3339(), ts.to_rfc3339());
    }

    #[test]
    fn a_malformed_cursor_is_rejected_not_panicked_on() {
        assert!(parse_cursor("not-a-cursor").is_err());
        assert!(parse_cursor("2026-09-13T00:00:00Z_not-a-uuid").is_err());
        assert!(parse_cursor(&Uuid::new_v4().to_string()).is_err());
    }
}
