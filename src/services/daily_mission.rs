use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::xp;

// §6.8's own example — the fixed set of skill_category values this
// mission tracks. Not every skill_category counts: reading/writing/
// pronunciation are deliberately excluded, matching the source exactly.
const TARGET: [(&str, i64); 4] = [("vocabulary", 5), ("grammar", 1), ("listening", 1), ("speaking", 1)];
const REWARD_XP: i32 = 80;

fn empty_progress() -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = TARGET.iter().map(|(k, _)| (k.to_string(), serde_json::json!(0))).collect();
    serde_json::Value::Object(map)
}

fn all_targets_met(progress: &serde_json::Map<String, serde_json::Value>) -> bool {
    TARGET.iter().all(|(skill, target)| progress.get(*skill).and_then(|v| v.as_i64()).unwrap_or(0) >= *target)
}

fn to_date_string(date: DateTime<Utc>) -> String {
    date.format("%Y-%m-%d").to_string()
}

struct MissionRow {
    progress: serde_json::Value,
    reward_claimed: bool,
}

async fn find(pool: &PgPool, user_id: Uuid, mission_date: &str) -> Result<Option<MissionRow>, AppError> {
    let row = sqlx::query_as!(
        MissionRow,
        r#"select progress, reward_claimed from user_daily_missions where user_id = $1 and mission_date = $2"#,
        user_id,
        mission_date,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn upsert(pool: &PgPool, user_id: Uuid, mission_date: &str, progress: &serde_json::Value, reward_claimed: bool) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into user_daily_missions (user_id, mission_date, progress, reward_claimed) values ($1, $2, $3, $4)
           on conflict (user_id, mission_date) do update set progress = $3, reward_claimed = $4"#,
        user_id,
        mission_date,
        progress,
        reward_claimed,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Called from the same hook sites as xp::award_xp/streak::record_activity
// (deferred — see R6 ticket notes). No-op for any skill_category
// outside TARGET.
pub async fn record_progress(pool: &PgPool, user_id: Uuid, skill_category: Option<&str>, today: DateTime<Utc>) -> Result<(), AppError> {
    let Some(skill_category) = skill_category else { return Ok(()) };
    if !TARGET.iter().any(|(k, _)| *k == skill_category) {
        return Ok(());
    }

    let date_str = to_date_string(today);
    let existing = find(pool, user_id, &date_str).await?;
    let mut progress = match &existing {
        Some(e) => e.progress.as_object().cloned().unwrap_or_default(),
        None => empty_progress().as_object().cloned().unwrap(),
    };
    let current = progress.get(skill_category).and_then(|v| v.as_i64()).unwrap_or(0);
    progress.insert(skill_category.to_string(), serde_json::json!(current + 1));

    let already_claimed = existing.as_ref().map(|e| e.reward_claimed).unwrap_or(false);
    let now_complete = !already_claimed && all_targets_met(&progress);

    if now_complete {
        // reference keyed to (user_id, date) — the XP ledger's own
        // idempotency prevents a double payout even on a race.
        xp::award_xp(pool, user_id, REWARD_XP, "daily_mission_completed", Some(&format!("daily_mission:{user_id}:{date_str}")), None).await?;
    }

    upsert(pool, user_id, &date_str, &serde_json::Value::Object(progress), already_claimed || now_complete).await
}

#[derive(Debug, serde::Serialize)]
pub struct DailyMissionSummary {
    pub date: String,
    pub progress: serde_json::Value,
    pub target: serde_json::Value,
    pub reward_claimed: bool,
}

// GET /me/daily-mission
pub async fn get_daily_mission(pool: &PgPool, user_id: Uuid, today: DateTime<Utc>) -> Result<DailyMissionSummary, AppError> {
    let date_str = to_date_string(today);
    let existing = find(pool, user_id, &date_str).await?;
    let target: serde_json::Map<String, serde_json::Value> = TARGET.iter().map(|(k, v)| (k.to_string(), serde_json::json!(v))).collect();
    Ok(DailyMissionSummary {
        date: date_str,
        progress: existing.as_ref().map(|e| e.progress.clone()).unwrap_or_else(empty_progress),
        target: serde_json::Value::Object(target),
        reward_claimed: existing.map(|e| e.reward_claimed).unwrap_or(false),
    })
}
