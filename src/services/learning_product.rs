use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::tutor;

#[derive(Debug, serde::Serialize)]
pub struct ProductResponse {
    pub id: Uuid,
    pub tutor_id: Uuid,
    pub r#type: String,
    pub title: String,
    pub description: String,
    pub price_idr: i64,
    pub capacity: Option<i32>,
    pub status: String,
    pub delivery_mode: String,
    pub session_count: Option<i32>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProductListResponse {
    pub items: Vec<ProductResponse>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProductPageResponse {
    pub items: Vec<ProductResponse>,
    pub next_cursor: Option<Uuid>,
}

const DELIVERY_MODES: [&str; 5] = ["live_class", "self_paced", "bootcamp", "hybrid", "package"];

fn validate_product_input(r#type: &str, price_idr: i64, capacity: Option<i32>, delivery_mode: &str, session_count: Option<i32>) -> Result<(), AppError> {
    if r#type != "private" && r#type != "group" {
        return Err(AppError::UnprocessableEntity("invalid_product_type", "type must be 'private' or 'group'".to_string()));
    }
    if price_idr < 0 {
        return Err(AppError::UnprocessableEntity("invalid_price", "price_idr must be a non-negative integer".to_string()));
    }
    if r#type == "private" && capacity.is_some() {
        return Err(AppError::UnprocessableEntity("invalid_capacity", "private products must not set a capacity".to_string()));
    }
    if r#type == "group" && !capacity.map(|c| c > 0).unwrap_or(false) {
        return Err(AppError::UnprocessableEntity("invalid_capacity", "group products require capacity > 0".to_string()));
    }
    if !DELIVERY_MODES.contains(&delivery_mode) {
        return Err(AppError::UnprocessableEntity("invalid_delivery_mode", format!("delivery_mode must be one of {}", DELIVERY_MODES.join(", "))));
    }
    if delivery_mode == "package" && !session_count.map(|s| s > 0).unwrap_or(false) {
        return Err(AppError::UnprocessableEntity("invalid_session_count", "package products require session_count > 0".to_string()));
    }
    if delivery_mode != "package" && session_count.is_some() {
        return Err(AppError::UnprocessableEntity("invalid_session_count", "session_count is only valid for delivery_mode='package'".to_string()));
    }
    Ok(())
}

// Exposed for cohort.rs/enrollment.rs/etc. — every other R7 module that
// needs to load the product a cohort belongs to.
pub async fn get_product_row(pool: &PgPool, id: Uuid) -> Result<Option<ProductResponse>, AppError> {
    let row = sqlx::query_as!(
        ProductResponse,
        r#"select id, tutor_id, type, title, description, price_idr, capacity, status, delivery_mode, session_count
           from learning_products where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// POST /tutors/me/products. Order matters: permission -> input
// validation -> lazy tutor-profile provisioning -> insert. The lazy
// provisioning exists because learning_products.tutor_id FKs
// tutor_profiles.user_id (not users.id), and platform_admin (who also
// has learning_product:create) normally has no tutor_profiles row.
#[allow(clippy::too_many_arguments)]
pub async fn create_product(
    pool: &PgPool,
    ctx: &AuthContext,
    r#type: &str,
    title: &str,
    description: &str,
    price_idr: i64,
    capacity: Option<i32>,
    delivery_mode: &str,
    session_count: Option<i32>,
) -> Result<ProductResponse, AppError> {
    require_permission(ctx, Resource::LearningProduct, Action::Create)?;
    validate_product_input(r#type, price_idr, capacity, delivery_mode, session_count)?;

    let existing_profile = tutor::find_profile_row(pool, ctx.user_id).await?;
    if existing_profile.is_none() {
        let Some(organization_id) = ctx.organization_id else { return Err(AppError::Forbidden) };
        tutor::assign_tutor_role(pool, ctx.user_id, organization_id).await?;
    }

    let row = sqlx::query_as!(
        ProductResponse,
        r#"insert into learning_products (tutor_id, type, title, description, price_idr, capacity, delivery_mode, session_count)
           values ($1, $2, $3, $4, $5, $6, $7, $8)
           returning id, tutor_id, type, title, description, price_idr, capacity, status, delivery_mode, session_count"#,
        ctx.user_id,
        r#type,
        title,
        description,
        price_idr,
        capacity,
        delivery_mode,
        session_count,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /tutors/{id}/products
pub async fn list_tutor_products(pool: &PgPool, ctx: &AuthContext, tutor_id: Uuid) -> Result<ProductListResponse, AppError> {
    let is_owner = ctx.user_id == tutor_id;
    let rows = if is_owner {
        sqlx::query_as!(
            ProductResponse,
            r#"select id, tutor_id, type, title, description, price_idr, capacity, status, delivery_mode, session_count
               from learning_products where tutor_id = $1 order by created_at"#,
            tutor_id,
        )
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as!(
            ProductResponse,
            r#"select id, tutor_id, type, title, description, price_idr, capacity, status, delivery_mode, session_count
               from learning_products where tutor_id = $1 and status = 'published' order by created_at"#,
            tutor_id,
        )
        .fetch_all(pool)
        .await?
    };
    Ok(ProductListResponse { items: rows })
}

// GET /products/{id} — draft/archived of someone else is 404, never 403.
pub async fn get_product(pool: &PgPool, ctx: &AuthContext, product_id: Uuid) -> Result<ProductResponse, AppError> {
    let product = get_product_row(pool, product_id).await?;
    match product {
        Some(p) if p.status == "published" || p.tutor_id == ctx.user_id => Ok(p),
        _ => Err(AppError::NotFound("learning_product_not_found")),
    }
}

// POST /products/{id}/publish — ownership-only, NOT the publish_flow.rs
// state machine (products have no in_review/reject path). Idempotent:
// already-published returns as-is. No role check at all.
pub async fn publish_product(pool: &PgPool, ctx: &AuthContext, product_id: Uuid) -> Result<ProductResponse, AppError> {
    let product = get_product_row(pool, product_id).await?;
    let Some(product) = product else { return Err(AppError::NotFound("learning_product_not_found")) };
    if product.tutor_id != ctx.user_id {
        return Err(AppError::NotFound("learning_product_not_found"));
    }
    if product.status == "published" {
        return Ok(product);
    }
    let row = sqlx::query_as!(
        ProductResponse,
        r#"update learning_products set status = 'published' where id = $1
           returning id, tutor_id, type, title, description, price_idr, capacity, status, delivery_mode, session_count"#,
        product_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /products?cursor=&limit= — keyset pagination on id (UUID
// ordering). Popped extra row's id becomes next_cursor; query uses
// strict `>` so the next page starts at that row.
pub async fn list_published_products(pool: &PgPool, cursor: Option<Uuid>, limit: Option<i64>) -> Result<ProductPageResponse, AppError> {
    let limit = limit.unwrap_or(20).clamp(1, 50);
    let mut rows = sqlx::query_as!(
        ProductResponse,
        r#"select id, tutor_id, type, title, description, price_idr, capacity, status, delivery_mode, session_count
           from learning_products
           where status = 'published' and ($1::uuid is null or id > $1)
           order by id asc
           limit $2"#,
        cursor,
        limit + 1,
    )
    .fetch_all(pool)
    .await?;

    let next_cursor = if rows.len() > limit as usize { rows.pop().map(|r| r.id) } else { None };
    Ok(ProductPageResponse { items: rows, next_cursor })
}
