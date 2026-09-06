use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::payment_provider::PaymentProvider;
use crate::services::wallet;

const TUTOR_SHARE: f64 = 0.7;

#[derive(Debug, serde::Serialize, Clone)]
pub struct OrderResponse {
    pub id: Uuid,
    pub enrollment_id: Uuid,
    pub amount_idr: i64,
    pub status: String,
    pub payment_id: Option<String>,
}

pub async fn find_by_enrollment_id(pool: &PgPool, enrollment_id: Uuid) -> Result<Option<OrderResponse>, AppError> {
    let row = sqlx::query_as!(
        OrderResponse,
        r#"select id, enrollment_id, amount_idr, status, payment_id from orders where enrollment_id = $1"#,
        enrollment_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn find_by_payment_id(pool: &PgPool, payment_id: &str) -> Result<Option<OrderResponse>, AppError> {
    let row = sqlx::query_as!(
        OrderResponse,
        r#"select id, enrollment_id, amount_idr, status, payment_id from orders where payment_id = $1"#,
        payment_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> Result<OrderResponse, AppError> {
    let row = sqlx::query_as!(
        OrderResponse,
        r#"update orders set status = $2 where id = $1 returning id, enrollment_id, amount_idr, status, payment_id"#,
        id,
        status,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

struct OrderContext {
    enrollment: crate::services::enrollment::EnrollmentResponse,
    product: crate::services::learning_product::ProductResponse,
}

async fn load_order_context(pool: &PgPool, enrollment_id: Uuid) -> Result<OrderContext, AppError> {
    let enrollment = crate::services::enrollment::find_by_id(pool, enrollment_id).await?.ok_or(AppError::NotFound("enrollment_not_found"))?;
    let cohort = crate::services::cohort::find_by_id(pool, enrollment.cohort_id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("enrollment references a missing cohort")))?;
    let product = crate::services::learning_product::get_product_row(pool, cohort.product_id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("cohort references a missing learning_product")))?;
    Ok(OrderContext { enrollment, product })
}

pub struct CheckoutResult {
    pub order: OrderResponse,
    pub qris_payload: String,
}

// POST /enrollments/{id}/checkout. amount_idr is always the product's
// own price server-side, never from the body. Insert-then-update in 2
// statements, not transactional — a provider failure leaves an orphan
// pending order with payment_id NULL, which permanently blocks
// re-checkout (UNIQUE on enrollment_id) — accepted MVP limitation,
// reproduced as-is.
pub async fn checkout(pool: &PgPool, ctx: &AuthContext, payment_provider: &dyn PaymentProvider, enrollment_id: Uuid) -> Result<CheckoutResult, AppError> {
    let context = load_order_context(pool, enrollment_id).await?;
    if context.enrollment.student_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }

    if find_by_enrollment_id(pool, enrollment_id).await?.is_some() {
        return Err(AppError::Conflict("order_already_exists"));
    }

    let order = sqlx::query_as!(
        OrderResponse,
        r#"insert into orders (enrollment_id, amount_idr) values ($1, $2)
           returning id, enrollment_id, amount_idr, status, payment_id"#,
        enrollment_id,
        context.product.price_idr,
    )
    .fetch_one(pool)
    .await?;

    let payment = payment_provider.create_payment(order.id, context.product.price_idr).await?;
    let updated = sqlx::query_as!(
        OrderResponse,
        r#"update orders set payment_id = $2 where id = $1 returning id, enrollment_id, amount_idr, status, payment_id"#,
        order.id,
        payment.payment_id,
    )
    .fetch_one(pool)
    .await?;

    Ok(CheckoutResult { order: updated, qris_payload: payment.qris_payload })
}

// POST /payments/{payment_id}/webhook — deliberately PUBLIC (see
// handlers::order for the auth-free wiring): a real provider's server
// wouldn't carry a user's bearer token, it'd authenticate via a
// provider-specific webhook signature that doesn't exist to verify yet.
// Idempotent: a 2nd call for an already-paid order is a no-op, not a
// 2nd payout.
pub async fn handle_webhook(pool: &PgPool, payment_id: &str) -> Result<OrderResponse, AppError> {
    let order = find_by_payment_id(pool, payment_id).await?.ok_or(AppError::NotFound("order_not_found"))?;
    if order.status != "pending" {
        return Ok(order);
    }

    let context = load_order_context(pool, order.enrollment_id).await?;

    let updated = set_status(pool, order.id, "paid").await?;
    crate::services::enrollment::set_status(pool, context.enrollment.id, "active").await?;

    // 30/70 split. Tutor's share is ROUNDED, the platform fee is
    // whatever's left over, so the two always sum back to amount_idr
    // exactly. Platform has no `users` row to be a 2nd ledger line, so
    // the fee is recorded as metadata on the same line.
    let tutor_share = (order.amount_idr as f64 * TUTOR_SHARE).round() as i64;
    let platform_fee = order.amount_idr - tutor_share;
    wallet::record_payout(pool, context.product.tutor_id, tutor_share, &format!("order:{};platform_fee_idr:{platform_fee}", order.id)).await?;

    Ok(updated)
}
