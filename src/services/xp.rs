use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

// §6.2's example XP table — documented in the source itself as
// "contoh, final angka lewat tuning", not a locked business number.
pub fn skill_xp(skill_category: &str) -> Option<i32> {
    match skill_category {
        "vocabulary" => Some(5),
        "grammar" => Some(10),
        "reading" => Some(10),
        "listening" => Some(10),
        "writing" => Some(20),
        "speaking" => Some(15),
        "pronunciation" => Some(15),
        _ => None,
    }
}

// A question with no skill_category tagged at all — smallest tier
// rather than 0, still real practice.
pub const UNTAGGED_QUESTION_XP: i32 = 5;

#[derive(Debug, serde::Serialize)]
pub struct XpEventSummary {
    pub amount: i64,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct XpSummary {
    pub total: i64,
    pub recent: Vec<XpEventSummary>,
}

pub async fn get_total(pool: &PgPool, user_id: Uuid) -> Result<i64, AppError> {
    let total = sqlx::query_scalar!(r#"select total from user_xp where user_id = $1"#, user_id).fetch_optional(pool).await?;
    Ok(total.unwrap_or(0))
}

pub async fn get_total_by_skill_category(pool: &PgPool, user_id: Uuid, skill_category: &str) -> Result<i64, AppError> {
    let total = sqlx::query_scalar!(
        r#"select coalesce(sum(amount), 0) as "total!" from xp_events where user_id = $1 and skill_category = $2"#,
        user_id,
        skill_category,
    )
    .fetch_one(pool)
    .await?;
    Ok(total)
}

// Idempotent when `reference` is given (a duplicate reference is a
// no-op, matching a caller retry). Returns whether XP was actually
// awarded (false = duplicate, skipped).
pub async fn award_xp(
    pool: &PgPool,
    user_id: Uuid,
    amount: i32,
    reason: &str,
    reference: Option<&str>,
    skill_category: Option<&str>,
) -> Result<bool, AppError> {
    let mut tx = pool.begin().await?;

    let inserted_id = sqlx::query_scalar!(
        r#"insert into xp_events (user_id, amount, reason, reference, skill_category)
           values ($1, $2, $3, $4, $5)
           on conflict (reference) do nothing
           returning id"#,
        user_id,
        amount,
        reason,
        reference,
        skill_category,
    )
    .fetch_optional(&mut *tx)
    .await?;

    if inserted_id.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    let amount = amount as i64;
    sqlx::query!(
        r#"insert into user_xp (user_id, total) values ($1, $2)
           on conflict (user_id) do update set total = user_xp.total + $2, updated_at = now()"#,
        user_id,
        amount,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

// GET /me/xp
pub async fn get_xp_summary(pool: &PgPool, user_id: Uuid) -> Result<XpSummary, AppError> {
    let total = get_total(pool, user_id).await?;
    let recent = sqlx::query_as!(
        XpEventSummary,
        r#"select amount, reason, created_at from xp_events where user_id = $1 order by created_at desc limit 20"#,
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(XpSummary { total, recent })
}

pub struct WeeklyTotal {
    pub user_id: Uuid,
    pub name: String,
    pub weekly_xp: i64,
}

pub async fn find_weekly_totals(pool: &PgPool, since: DateTime<Utc>) -> Result<Vec<WeeklyTotal>, AppError> {
    let rows = sqlx::query_as!(
        WeeklyTotal,
        r#"select xe.user_id as "user_id!", u.name as "name!", sum(xe.amount) as "weekly_xp!"
           from xp_events xe inner join users u on u.id = xe.user_id
           where xe.created_at >= $1
           group by xe.user_id, u.name
           order by sum(xe.amount) desc"#,
        since,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_weekly_event_count(pool: &PgPool, user_id: Uuid, since: DateTime<Utc>) -> Result<i64, AppError> {
    let count = sqlx::query_scalar!(
        r#"select count(*) as "count!" from xp_events where user_id = $1 and created_at >= $2"#,
        user_id,
        since,
    )
    .fetch_one(pool)
    .await?;
    Ok(count)
}
