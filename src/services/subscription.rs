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

// Monthly price in rupiah. Same code-side registry idiom as
// quiz_subtype/block_schema: a tier is validated by matching here, not
// by a DB CHECK, so adding a tier is a code change and never a
// migration.
pub fn tier_price_idr(tier: &str) -> Option<i64> {
    match tier {
        "plus" => Some(29_000),
        "pro" => Some(79_000),
        _ => None,
    }
}

#[derive(Debug, serde::Serialize)]
pub struct TierBenefit {
    pub tier: String,
    pub price_idr: i64,
    pub diamond_allowance: i64,
    pub benefits: Vec<&'static str>,
}

// GET /subscriptions/tiers — open catalogue, readable before a visitor
// signs in so the paywall can be shown without an account.
pub fn list_tiers() -> Vec<TierBenefit> {
    ["plus", "pro"]
        .iter()
        .map(|t| TierBenefit {
            tier: (*t).to_string(),
            price_idr: tier_price_idr(t).unwrap_or(0),
            diamond_allowance: tier_allowance(t).unwrap_or(0),
            benefits: match *t {
                "plus" => vec![
                    "Belajar tanpa iklan",
                    "100 Diamond setiap bulan",
                    "Kuis AI & feedback tanpa batas",
                    "Akses semua learning path",
                ],
                _ => vec![
                    "Belajar tanpa iklan",
                    "300 Diamond setiap bulan",
                    "Kuis AI & feedback tanpa batas",
                    "Akses semua learning path & unduh materi",
                    "Statistik progres lanjutan",
                ],
            },
        })
        .collect()
}

// One place the app asks "what is this user allowed to do", so the
// paywall, the ad slots and the AI gateway all agree. Derived from the
// subscription rather than stored, so it can never drift out of sync
// with an expired period.
#[derive(Debug, serde::Serialize)]
pub struct Entitlements {
    pub tier: Option<String>,
    pub active: bool,
    pub ad_free: bool,
    pub unlimited_ai: bool,
    pub all_learning_paths: bool,
    pub advanced_stats: bool,
    pub downloads: bool,
}

// GET /me/entitlements
pub async fn get_entitlements(pool: &PgPool, ctx: &AuthContext) -> Result<Entitlements, AppError> {
    let row = sqlx::query!(
        r#"select tier, status, current_period_end from subscriptions where user_id = $1"#,
        ctx.user_id,
    )
    .fetch_optional(pool)
    .await?;

    let active = matches!(&row, Some(r) if r.status == "active" && r.current_period_end > Utc::now());
    let tier = row.map(|r| r.tier).filter(|_| active);
    let is_pro = tier.as_deref() == Some("pro");

    Ok(Entitlements {
        active,
        ad_free: active,
        unlimited_ai: active,
        all_learning_paths: active,
        // Pro-only extras, matching list_tiers()'s benefit copy.
        advanced_stats: is_pro,
        downloads: is_pro,
        tier,
    })
}

// Whether a user's subscription entitles them to ad-free study. Used by
// the ad service and by the frontend to decide if an ad slot renders at
// all; every paid tier is ad-free.
pub async fn is_ad_free(pool: &PgPool, user_id: Uuid) -> Result<bool, AppError> {
    let row = sqlx::query!(
        r#"select status, current_period_end from subscriptions where user_id = $1"#,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(matches!(row, Some(r) if r.status == "active" && r.current_period_end > Utc::now()))
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

// Activates a tier for `user_id`. Called by order.rs's webhook once a
// QRIS payment settles — NOT reachable directly from a route any more,
// which is what stops a subscription from being granted without
// payment. Always grants exactly 1 allowance transaction per period.
pub async fn activate(pool: &PgPool, user_id: Uuid, tier: &str) -> Result<SubscriptionResponse, AppError> {
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
        user_id,
        tier,
        now,
        current_period_end,
    )
    .fetch_one(pool)
    .await?;

    economy::ensure_credits_row(pool, user_id).await?;
    let reference = format!("subscription:{}:{}", user_id, now.to_rfc3339());
    economy::grant_allowance(pool, user_id, allowance, &reference, expires_at).await?;

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
