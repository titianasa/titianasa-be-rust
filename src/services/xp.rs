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

/// XP for finishing a module quiz. Paid ONCE in full, the first time the
/// learner passes; afterwards a retake only pays a small amount when it
/// raises their best score. A failed attempt, or a retake that doesn't
/// beat the best, pays nothing — replaying the same quiz is no longer a
/// way to farm XP (it used to pay a flat 20 on every submit).
///
/// Size matters: "Latihan 50 soal" is worth five times "Latihan 10 soal".
#[derive(Debug, Clone, PartialEq)]
pub enum QuizXp {
    FirstPass { amount: i32 },
    Improved { amount: i32, new_best: i64 },
}

pub const QUIZ_XP_PER_QUESTION: i32 = 2;
pub const DEFAULT_PASSING_SCORE: f64 = 70.0;

pub fn quiz_xp(question_count: usize, passing_score: f64, previous_best: Option<f64>, score: f64) -> Option<QuizXp> {
    let base = QUIZ_XP_PER_QUESTION * question_count as i32;
    if base == 0 {
        return None;
    }
    let passed_before = previous_best.is_some_and(|b| b >= passing_score);
    if !passed_before {
        return (score >= passing_score).then_some(QuizXp::FirstPass { amount: base });
    }
    let best = previous_best.unwrap_or(0.0);
    if score <= best {
        return None;
    }
    let amount = ((base as f64) * 0.2 * (score - best) / 100.0).round().max(1.0) as i32;
    Some(QuizXp::Improved { amount, new_best: score.floor() as i64 })
}

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

#[cfg(test)]
mod quiz_xp_tests {
    use super::*;

    #[test]
    fn the_first_pass_pays_in_full_scaled_by_paper_size() {
        assert_eq!(quiz_xp(10, 70.0, None, 80.0), Some(QuizXp::FirstPass { amount: 20 }));
        assert_eq!(quiz_xp(50, 70.0, Some(40.0), 70.0), Some(QuizXp::FirstPass { amount: 100 }), "earlier failures don't use up the first pass");
    }

    #[test]
    fn failing_pays_nothing() {
        assert_eq!(quiz_xp(25, 70.0, None, 69.9), None);
    }

    #[test]
    fn a_retake_pays_only_when_it_beats_the_best() {
        assert_eq!(quiz_xp(10, 70.0, Some(80.0), 80.0), None, "same score again is not progress");
        assert_eq!(quiz_xp(10, 70.0, Some(80.0), 60.0), None);
        // 20 base × 0.2 × 20 points = 0.8 → rounds up to the 1 XP floor.
        assert_eq!(quiz_xp(10, 70.0, Some(80.0), 100.0), Some(QuizXp::Improved { amount: 1, new_best: 100 }));
        assert_eq!(quiz_xp(50, 70.0, Some(70.0), 100.0), Some(QuizXp::Improved { amount: 6, new_best: 100 }));
    }
}
