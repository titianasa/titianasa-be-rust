use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::{streak, xp};
use crate::Config;

const WINDOW_DAYS: i64 = 7;
const STREAK_CAP_DAYS: f64 = 30.0;
const WEEKLY_ACTIVITY_CAP: f64 = 50.0;

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LeagueTier {
    Bronze,
    Silver,
    Gold,
    Platinum,
    Diamond,
    Master,
}

fn tier_for_score(score: f64) -> LeagueTier {
    if score >= 90.0 {
        LeagueTier::Master
    } else if score >= 75.0 {
        LeagueTier::Diamond
    } else if score >= 60.0 {
        LeagueTier::Platinum
    } else if score >= 40.0 {
        LeagueTier::Gold
    } else if score >= 20.0 {
        LeagueTier::Silver
    } else {
        LeagueTier::Bronze
    }
}

#[derive(Debug, serde::Serialize)]
pub struct LeagueInputs {
    pub current_streak: i32,
    pub average_mastery: f64,
    pub weekly_activity_count: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct LeagueSummary {
    pub tier: LeagueTier,
    pub score: f64,
    pub inputs: LeagueInputs,
}

async fn find_average_score(pool: &PgPool, user_id: Uuid, confidence_threshold: f64) -> Result<Option<f64>, AppError> {
    let average = sqlx::query_scalar!(
        r#"select avg(score) as "average" from masteries where user_id = $1 and confidence >= $2"#,
        user_id,
        confidence_threshold,
    )
    .fetch_one(pool)
    .await?;
    Ok(average)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

// GET /me/league — tier is a WEIGHTED FORMULA (streak + mastery +
// weekly activity), never raw XP ("bukan cuma XP, supaya tidak bisa
// dibeli"). Computed on-read every call, never stored/refreshed by a
// cron job — no scheduling infra exists, and the 3 underlying queries
// are cheap at this project's scale.
pub async fn get_league(pool: &PgPool, config: &Config, user_id: Uuid) -> Result<LeagueSummary, AppError> {
    let since = chrono::Utc::now() - chrono::Duration::days(WINDOW_DAYS);

    let current_streak = streak::find_current_streak(pool, user_id).await?;
    let average_mastery_raw = find_average_score(pool, user_id, config.mastery_confidence_threshold).await?;
    let weekly_activity_count = xp::get_weekly_event_count(pool, user_id, since).await?;

    // No confident mastery data yet defaults to 0, not "skip this
    // input" — a user with no real ability signal shouldn't get a free
    // pass on the mastery component.
    let average_mastery = average_mastery_raw.unwrap_or(0.0);

    let streak_component = (current_streak as f64).min(STREAK_CAP_DAYS) / STREAK_CAP_DAYS * 100.0;
    let activity_component = (weekly_activity_count as f64).min(WEEKLY_ACTIVITY_CAP) / WEEKLY_ACTIVITY_CAP * 100.0;
    let score = (streak_component + average_mastery + activity_component) / 3.0;

    Ok(LeagueSummary {
        tier: tier_for_score(score),
        score: round1(score),
        inputs: LeagueInputs { current_streak, average_mastery: round1(average_mastery), weekly_activity_count },
    })
}
