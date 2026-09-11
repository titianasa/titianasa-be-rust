use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::payment_provider::PaymentProvider;
use crate::services::wallet;

const TUTOR_SHARE: f64 = 0.7;

// Migration 0040 — an order now settles either a marketplace class
// enrollment or a subscription, so enrollment_id is optional and `kind`
// says which fields are meaningful. A DB CHECK enforces that each kind
// carries the fields it needs, so the branches below can rely on it.
#[derive(Debug, serde::Serialize, Clone)]
pub struct OrderResponse {
    pub id: Uuid,
    pub kind: String,
    pub enrollment_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub subscription_tier: Option<String>,
    pub amount_idr: i64,
    pub status: String,
    pub payment_id: Option<String>,
}

pub async fn find_by_enrollment_id(pool: &PgPool, enrollment_id: Uuid) -> Result<Option<OrderResponse>, AppError> {
    let row = sqlx::query_as!(
        OrderResponse,
        r#"select id, kind, enrollment_id, user_id, subscription_tier, amount_idr, status, payment_id from orders where enrollment_id = $1"#,
        enrollment_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn find_by_payment_id(pool: &PgPool, payment_id: &str) -> Result<Option<OrderResponse>, AppError> {
    let row = sqlx::query_as!(
        OrderResponse,
        r#"select id, kind, enrollment_id, user_id, subscription_tier, amount_idr, status, payment_id from orders where payment_id = $1"#,
        payment_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> Result<OrderResponse, AppError> {
    let row = sqlx::query_as!(
        OrderResponse,
        r#"update orders set status = $2 where id = $1 returning id, kind, enrollment_id, user_id, subscription_tier, amount_idr, status, payment_id"#,
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
        r#"insert into orders (kind, enrollment_id, amount_idr) values ('enrollment', $1, $2)
           returning id, kind, enrollment_id, user_id, subscription_tier, amount_idr, status, payment_id"#,
        enrollment_id,
        context.product.price_idr,
    )
    .fetch_one(pool)
    .await?;

    let payment = payment_provider.create_payment(order.id, context.product.price_idr).await?;
    let updated = sqlx::query_as!(
        OrderResponse,
        r#"update orders set payment_id = $2 where id = $1 returning id, kind, enrollment_id, user_id, subscription_tier, amount_idr, status, payment_id"#,
        order.id,
        payment.payment_id,
    )
    .fetch_one(pool)
    .await?;

    Ok(CheckoutResult { order: updated, qris_payload: payment.qris_payload })
}

// POST /subscriptions/checkout — the paywall's entry point. Mirrors the
// enrollment checkout exactly (insert order, ask the provider for a
// QRIS payload, store payment_id), so both settle through the same
// webhook. Price comes from the tier registry server-side, never from
// the request body.
pub async fn checkout_subscription(
    pool: &PgPool,
    ctx: &AuthContext,
    payment_provider: &dyn PaymentProvider,
    tier: &str,
) -> Result<CheckoutResult, AppError> {
    let Some(amount_idr) = crate::services::subscription::tier_price_idr(tier) else {
        return Err(AppError::UnprocessableEntity(
            "invalid_tier",
            r#"tier harus "plus" atau "pro""#.to_string(),
        ));
    };

    // A user may retry checkout, so an abandoned pending order must not
    // block them forever the way the enrollment path's UNIQUE does.
    // Supersede any earlier pending subscription order instead.
    sqlx::query!(
        r#"update orders set status = 'cancelled'
           where kind = 'subscription' and user_id = $1 and status = 'pending'"#,
        ctx.user_id,
    )
    .execute(pool)
    .await?;

    let order = sqlx::query_as!(
        OrderResponse,
        r#"insert into orders (kind, user_id, subscription_tier, amount_idr)
           values ('subscription', $1, $2, $3)
           returning id, kind, enrollment_id, user_id, subscription_tier, amount_idr, status, payment_id"#,
        ctx.user_id,
        tier,
        amount_idr,
    )
    .fetch_one(pool)
    .await?;

    let payment = payment_provider.create_payment(order.id, amount_idr).await?;
    let updated = sqlx::query_as!(
        OrderResponse,
        r#"update orders set payment_id = $2 where id = $1
           returning id, kind, enrollment_id, user_id, subscription_tier, amount_idr, status, payment_id"#,
        order.id,
        payment.payment_id,
    )
    .fetch_one(pool)
    .await?;

    Ok(CheckoutResult { order: updated, qris_payload: payment.qris_payload })
}

// GET /orders/{id} — lets the paywall poll until the webhook settles the
// QRIS payment, since the provider confirms out-of-band.
pub async fn get_own_order(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<OrderResponse, AppError> {
    let order = sqlx::query_as!(
        OrderResponse,
        r#"select id, kind, enrollment_id, user_id, subscription_tier, amount_idr, status, payment_id
           from orders where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("order_not_found"))?;

    let owner = match order.kind.as_str() {
        "subscription" => order.user_id,
        _ => match order.enrollment_id {
            Some(eid) => crate::services::enrollment::find_by_id(pool, eid).await?.map(|e| e.student_id),
            None => None,
        },
    };
    if owner != Some(ctx.user_id) {
        return Err(AppError::Forbidden);
    }
    Ok(order)
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

    // A subscription order has no enrollment and no tutor to pay out —
    // settling it just activates the tier. Returns early so the
    // marketplace revenue split below stays enrollment-only.
    if order.kind == "subscription" {
        let (Some(user_id), Some(tier)) = (order.user_id, order.subscription_tier.as_deref()) else {
            return Err(AppError::Internal(anyhow::anyhow!("subscription order missing user_id/tier")));
        };
        let updated = set_status(pool, order.id, "paid").await?;
        crate::services::subscription::activate(pool, user_id, tier).await?;
        return Ok(updated);
    }

    let enrollment_id = order
        .enrollment_id
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("enrollment order missing enrollment_id")))?;
    let context = load_order_context(pool, enrollment_id).await?;

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
