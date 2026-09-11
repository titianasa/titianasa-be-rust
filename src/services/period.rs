use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::period::CreatePeriodRequest;
use crate::services::permissions::{require_permission_in_org, Action, Resource};

// Phase 36 — "Periode": see migrations/0036_periods.sql's header for
// why this is one flexibly-named entity rather than a rigid semester/
// batch-specific model.

#[derive(Debug, Clone, serde::Serialize)]
pub struct PeriodResponse {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct PeriodListResponse {
    pub items: Vec<PeriodResponse>,
}

fn parse_date(value: &Option<String>, field: &str) -> Result<Option<NaiveDate>, AppError> {
    match value {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .map_err(|_| AppError::UnprocessableEntity("invalid_date", format!("{field} must be YYYY-MM-DD"))),
    }
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<PeriodResponse>, AppError> {
    let row = sqlx::query_as!(
        PeriodResponse,
        r#"select id, organization_id, name, start_date, end_date, status, created_at from periods where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// POST /organizations/{id}/periods
pub async fn create(pool: &PgPool, ctx: &AuthContext, organization_id: Uuid, req: CreatePeriodRequest) -> Result<PeriodResponse, AppError> {
    require_permission_in_org(ctx, organization_id, Resource::Period, Action::Create)?;
    if req.name.trim().is_empty() {
        return Err(AppError::UnprocessableEntity("name_required", "nama periode wajib diisi".to_string()));
    }
    let start_date = parse_date(&req.start_date, "start_date")?;
    let end_date = parse_date(&req.end_date, "end_date")?;

    let row = sqlx::query_as!(
        PeriodResponse,
        r#"insert into periods (organization_id, name, start_date, end_date)
           values ($1, $2, $3, $4)
           returning id, organization_id, name, start_date, end_date, status, created_at"#,
        organization_id,
        req.name,
        start_date,
        end_date,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /organizations/{id}/periods
pub async fn list_for_org(pool: &PgPool, ctx: &AuthContext, organization_id: Uuid) -> Result<PeriodListResponse, AppError> {
    require_permission_in_org(ctx, organization_id, Resource::Period, Action::View)?;
    let rows = sqlx::query_as!(
        PeriodResponse,
        r#"select id, organization_id, name, start_date, end_date, status, created_at
           from periods where organization_id = $1 order by created_at desc"#,
        organization_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(PeriodListResponse { items: rows })
}

// PATCH /periods/{id} — v1 only toggles status (archive/reactivate); no
// rename yet, matching the scope actually asked for.
pub async fn set_status(pool: &PgPool, ctx: &AuthContext, id: Uuid, status: &str) -> Result<PeriodResponse, AppError> {
    let existing = find_by_id(pool, id).await?.ok_or(AppError::NotFound("period_not_found"))?;
    require_permission_in_org(ctx, existing.organization_id, Resource::Period, Action::Create)?;

    let row = sqlx::query_as!(
        PeriodResponse,
        r#"update periods set status = $2, updated_at = now() where id = $1
           returning id, organization_id, name, start_date, end_date, status, created_at"#,
        id,
        status,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}
