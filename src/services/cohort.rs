use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::learning_product::ProductResponse;

// The shared row-context authorization primitive used by nearly every
// cohort-management endpoint in this feature area (cohorts, complete,
// certificate issue, tutor-cancel, assignments, grading, gradebook).
// Boolean, not throwing — some callers fall through to a student-only
// branch on `false`. NOT a permissions.rs matrix case (row context, not
// role-only).
pub async fn can_manage_cohorts(pool: &PgPool, ctx: &AuthContext, product: &ProductResponse) -> Result<bool, AppError> {
    if ctx.role.as_deref() == Some("platform_admin") || ctx.user_id == product.tutor_id {
        return Ok(true);
    }
    if matches!(ctx.role.as_deref(), Some("org_owner") | Some("academic_director")) {
        let tutor_org = sqlx::query_scalar!(r#"select organization_id from tutor_profiles where user_id = $1"#, product.tutor_id)
            .fetch_optional(pool)
            .await?;
        if let Some(tutor_org) = tutor_org {
            if ctx.organization_id == Some(tutor_org) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[derive(Debug, serde::Serialize)]
pub struct CohortResponse {
    pub id: Uuid,
    pub product_id: Uuid,
    pub name: String,
    pub schedule: serde_json::Value,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub meeting_url: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct CohortListResponse {
    pub items: Vec<CohortResponse>,
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<CohortResponse>, AppError> {
    let row = sqlx::query_as!(
        CohortResponse,
        r#"select id, product_id, name, schedule, starts_at, ends_at, meeting_url from cohorts where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn find_product_for_cohort(pool: &PgPool, cohort: &CohortResponse) -> Result<ProductResponse, AppError> {
    crate::services::learning_product::get_product_row(pool, cohort.product_id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("cohort references a missing learning_product")))
}

// POST /products/{id}/cohorts
#[allow(clippy::too_many_arguments)]
pub async fn create_cohort(
    pool: &PgPool,
    ctx: &AuthContext,
    product_id: Uuid,
    name: &str,
    schedule: &serde_json::Value,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    meeting_url: Option<&str>,
) -> Result<CohortResponse, AppError> {
    let product =
        crate::services::learning_product::get_product_row(pool, product_id).await?.ok_or(AppError::NotFound("learning_product_not_found"))?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::Forbidden);
    }

    let row = sqlx::query_as!(
        CohortResponse,
        r#"insert into cohorts (product_id, name, schedule, starts_at, ends_at, meeting_url)
           values ($1, $2, $3, $4, $5, $6)
           returning id, product_id, name, schedule, starts_at, ends_at, meeting_url"#,
        product_id,
        name,
        schedule,
        starts_at,
        ends_at,
        meeting_url,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /products/{id}/cohorts — published product: anyone; else
// can_manage_cohorts else 404 (not 403).
pub async fn list_cohorts_for_product(pool: &PgPool, ctx: &AuthContext, product_id: Uuid) -> Result<CohortListResponse, AppError> {
    let product =
        crate::services::learning_product::get_product_row(pool, product_id).await?.ok_or(AppError::NotFound("learning_product_not_found"))?;
    if product.status != "published" && !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::NotFound("learning_product_not_found"));
    }
    let rows = sqlx::query_as!(
        CohortResponse,
        r#"select id, product_id, name, schedule, starts_at, ends_at, meeting_url from cohorts where product_id = $1"#,
        product_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(CohortListResponse { items: rows })
}

pub async fn find_product_for_cohort_id(pool: &PgPool, cohort_id: Uuid) -> Result<(CohortResponse, ProductResponse), AppError> {
    let cohort = find_by_id(pool, cohort_id).await?.ok_or(AppError::NotFound("cohort_not_found"))?;
    let product = find_product_for_cohort(pool, &cohort).await?;
    Ok((cohort, product))
}
