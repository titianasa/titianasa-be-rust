use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::economy;

// Example amounts from ADR-0005 itself ("angka ini contoh awal, bukan
// final bisnis") — tunable defaults.
fn tier_allowance(tier: &str) -> Option<i64> {
    match tier {
        "plus" => Some(100),
        "pro" => Some(300),
        _ => None,
    }
}

const PERIOD_DAYS: i64 = 30;
const GRACE_DAYS: i64 = 30; // ADR-0005: "expire di akhir periode berikutnya (grace 1 bulan)"

struct SubscriptionRow {
    #[allow(dead_code)]
    user_id: Uuid,
    tier: String,
    status: String,
    current_period_start: DateTime<Utc>,
    current_period_end: DateTime<Utc>,
}

// `subscribed: false` rather than a bare `null` body for "no
// subscription yet" — same "never return a bare null top-level
// response" convention as GET /mastery/{id}'s `message:
// "insufficient_data"` shape. No `user_id` field — the caller already
// knows who they are, this is always "my own" subscription.
#[derive(Debug, serde::Serialize)]
pub struct SubscriptionResponse {
    pub subscribed: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub details: Option<SubscriptionDetails>,
}

#[derive(Debug, serde::Serialize)]
pub struct SubscriptionDetails {
    pub tier: String,
    pub status: String,
    pub current_period_start: DateTime<Utc>,
    pub current_period_end: DateTime<Utc>,
}

fn to_response(row: SubscriptionRow) -> SubscriptionResponse {
    SubscriptionResponse {
        subscribed: true,
        details: Some(SubscriptionDetails { tier: row.tier, status: row.status, current_period_start: row.current_period_start, current_period_end: row.current_period_end }),
    }
}

// POST /subscriptions/subscribe. No real payment gateway gates this yet
// — subscribing activates immediately. Always grants exactly 1 new
// allowance transaction for the period being started.
pub async fn subscribe(pool: &PgPool, ctx: &AuthContext, tier: &str) -> Result<SubscriptionResponse, AppError> {
    let Some(allowance) = tier_allowance(tier) else {
        return Err(AppError::UnprocessableEntity("invalid_tier", r#"tier must be one of "plus", "pro""#.to_string()));
    };

    let now = Utc::now();
    let current_period_end = now + chrono::Duration::days(PERIOD_DAYS);
    let expires_at = current_period_end + chrono::Duration::days(GRACE_DAYS);

    let row = sqlx::query_as!(
        SubscriptionRow,
        r#"insert into subscriptions (user_id, tier, status, current_period_start, current_period_end)
           values ($1, $2, 'active', $3, $4)
           on conflict (user_id) do update set tier = $2, status = 'active', current_period_start = $3, current_period_end = $4
           returning user_id, tier, status, current_period_start, current_period_end"#,
        ctx.user_id,
        tier,
        now,
        current_period_end,
    )
    .fetch_one(pool)
    .await?;

    economy::ensure_credits_row(pool, ctx.user_id).await?;
    let reference = format!("subscription:{}:{}", ctx.user_id, now.to_rfc3339());
    economy::grant_allowance(pool, ctx.user_id, allowance, &reference, expires_at).await?;

    Ok(to_response(row))
}

// GET /subscriptions/me
pub async fn get_my_subscription(pool: &PgPool, ctx: &AuthContext) -> Result<SubscriptionResponse, AppError> {
    let row = sqlx::query_as!(
        SubscriptionRow,
        r#"select user_id, tier, status, current_period_start, current_period_end from subscriptions where user_id = $1"#,
        ctx.user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(to_response).unwrap_or(SubscriptionResponse { subscribed: false, details: None }))
}
