use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;

// P11-004 — no real ad SDK integrated (matches Bun's StubAdProvider):
// this simulates "the ad finished playing" instantly, no network call,
// no real ad ever shown. When a real SDK is integrated later, this is
// the seam to swap.
#[derive(Debug, serde::Serialize)]
pub struct WatchAdResponse {
    pub view_id: Uuid,
}

// POST /ads/watch — grants 1 consumable ad-view credit, spendable by
// the AI Gateway as an alternative to a diamond charge.
pub async fn watch_ad(pool: &PgPool, ctx: &AuthContext) -> Result<WatchAdResponse, AppError> {
    let view_id = sqlx::query_scalar!(r#"insert into ad_views (user_id) values ($1) returning id"#, ctx.user_id)
        .fetch_one(pool)
        .await?;
    Ok(WatchAdResponse { view_id })
}

pub struct AdView {
    pub id: Uuid,
}

// Oldest unused view first (FIFO) — arbitrary among unused rows, but
// deterministic rather than picking whichever the query planner returns.
pub async fn find_oldest_unused(pool: &PgPool, user_id: Uuid) -> Result<Option<AdView>, AppError> {
    let row = sqlx::query_as!(AdView, r#"select id from ad_views where user_id = $1 and consumed_at is null order by viewed_at asc limit 1"#, user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

// Guarded by `consumed_at IS NULL` so consuming the same view twice
// concurrently can't both succeed — the 2nd call's UPDATE matches 0
// rows and returns false instead of re-consuming.
pub async fn mark_consumed(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let result = sqlx::query!(r#"update ad_views set consumed_at = now() where id = $1 and consumed_at is null"#, id).execute(pool).await?;
    Ok(result.rows_affected() > 0)
}
