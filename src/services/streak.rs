use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

const FREEZE_BONUS_EVERY_DAYS: i32 = 7;

struct StreakRow {
    current_streak: i32,
    longest_streak: i32,
    freezes_available: i32,
    last_active_date: Option<String>,
}

async fn find(pool: &PgPool, user_id: Uuid) -> Result<Option<StreakRow>, AppError> {
    let row = sqlx::query_as!(
        StreakRow,
        r#"select current_streak, longest_streak, freezes_available, last_active_date from user_streaks where user_id = $1"#,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn upsert(
    pool: &PgPool,
    user_id: Uuid,
    current_streak: i32,
    longest_streak: i32,
    freezes_available: i32,
    last_active_date: &str,
) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into user_streaks (user_id, current_streak, longest_streak, freezes_available, last_active_date)
           values ($1, $2, $3, $4, $5)
           on conflict (user_id) do update set
             current_streak = $2, longest_streak = $3, freezes_available = $4, last_active_date = $5"#,
        user_id,
        current_streak,
        longest_streak,
        freezes_available,
        last_active_date,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn to_date_string(date: DateTime<Utc>) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn yesterday_of(date: DateTime<Utc>) -> String {
    to_date_string(date - chrono::Duration::days(1))
}

// Called once per "did something" hook, same call sites as
// xp::award_xp (P8-001) — see the checkAnswer/submitAttempt hook sites,
// deferred (see R6 ticket notes). `today` is a parameter (not read
// internally) so tests can simulate specific days.
pub async fn record_activity(pool: &PgPool, user_id: Uuid, today: DateTime<Utc>) -> Result<(), AppError> {
    let today_str = to_date_string(today);
    let existing = find(pool, user_id).await?;

    let Some(existing) = existing else {
        upsert(pool, user_id, 1, 1, 1, &today_str).await?;
        return Ok(());
    };

    // Already counted today — no-op, not a double-count.
    if existing.last_active_date.as_deref() == Some(today_str.as_str()) {
        return Ok(());
    }

    let mut new_freezes_available = existing.freezes_available;
    let new_current_streak;

    if existing.last_active_date.as_deref() == Some(yesterday_of(today).as_str()) {
        new_current_streak = existing.current_streak + 1;
    } else if existing.freezes_available > 0 {
        // Gap of 1+ days, but a freeze absorbs it — the streak count
        // itself stays exactly where it was.
        new_current_streak = existing.current_streak;
        new_freezes_available -= 1;
    } else {
        new_current_streak = 1;
    }

    // Only award a bonus freeze when the streak count genuinely just
    // advanced to a new 7-day multiple — not when a freeze was just
    // consumed or reset, which would otherwise re-trigger the milestone.
    let streak_advanced = new_current_streak > existing.current_streak;
    if streak_advanced && new_current_streak % FREEZE_BONUS_EVERY_DAYS == 0 {
        new_freezes_available += 1;
    }

    upsert(
        pool,
        user_id,
        new_current_streak,
        existing.longest_streak.max(new_current_streak),
        new_freezes_available,
        &today_str,
    )
    .await
}

#[derive(Debug, serde::Serialize)]
pub struct StreakSummary {
    pub current_streak: i32,
    pub longest_streak: i32,
    pub freezes_available: i32,
}

// GET /me/streak
pub async fn get_streak(pool: &PgPool, user_id: Uuid) -> Result<StreakSummary, AppError> {
    let row = find(pool, user_id).await?;
    Ok(match row {
        Some(r) => StreakSummary { current_streak: r.current_streak, longest_streak: r.longest_streak, freezes_available: r.freezes_available },
        None => StreakSummary { current_streak: 0, longest_streak: 0, freezes_available: 1 },
    })
}

// Exposed for league.rs's read of current_streak — mirrors
// streak_repository.find's own reuse pattern in league_service.ts.
pub async fn find_current_streak(pool: &PgPool, user_id: Uuid) -> Result<i32, AppError> {
    Ok(find(pool, user_id).await?.map(|r| r.current_streak).unwrap_or(0))
}

