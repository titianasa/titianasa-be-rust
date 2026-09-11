use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;

use crate::errors::AppError;
use crate::models::auth::AuthContext;

// Migration 0040 — the profile a visitor fills in BEFORE signing up.
//
// It is collected anonymously in the browser and pushed here exactly
// once, right after OAuth succeeds. That ordering is the point of the
// whole onboarding change: someone who never signs up leaves no row
// behind, and someone who does arrives with their goal already known,
// so the dashboard is personalised on their first visit rather than
// empty.

#[derive(Debug, serde::Serialize)]
pub struct LearningProfile {
    pub goal: Option<String>,
    pub jenjang: Option<String>,
    pub interests: Value,
    /// Curriculum labels the stated goal maps onto (`ujian:utbk-pm`,
    /// `jenjang:SMA-11`), so the dashboard can assemble a real path
    /// from the module library instead of guessing.
    pub target_labels: Value,
    pub daily_minutes: Option<i32>,
    pub onboarding_completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpsertProfileRequest {
    pub goal: Option<String>,
    pub jenjang: Option<String>,
    pub interests: Option<Value>,
    pub target_labels: Option<Value>,
    pub daily_minutes: Option<i32>,
}

fn validate(req: &UpsertProfileRequest) -> Result<(), AppError> {
    if let Some(m) = req.daily_minutes {
        if !(1..=1440).contains(&m) {
            return Err(AppError::UnprocessableEntity(
                "invalid_daily_minutes",
                "daily_minutes harus antara 1 dan 1440".to_string(),
            ));
        }
    }
    for (name, v) in [("interests", &req.interests), ("target_labels", &req.target_labels)] {
        if let Some(v) = v {
            if !v.is_array() {
                return Err(AppError::UnprocessableEntity("invalid_profile", format!("{name} harus berupa array")));
            }
        }
    }
    Ok(())
}

// PUT /me/learning-profile — full replace, called once at sign-in with
// whatever the browser collected, and again whenever the learner edits
// their profile. Upsert rather than insert: signing in on a second
// device must not fail.
pub async fn upsert(pool: &PgPool, ctx: &AuthContext, req: UpsertProfileRequest) -> Result<LearningProfile, AppError> {
    validate(&req)?;
    let interests = req.interests.unwrap_or_else(|| Value::Array(vec![]));
    let target_labels = req.target_labels.unwrap_or_else(|| Value::Array(vec![]));

    let row = sqlx::query_as!(
        LearningProfile,
        r#"insert into user_learning_profiles (user_id, goal, jenjang, interests, target_labels, daily_minutes)
           values ($1, $2, $3, $4, $5, $6)
           on conflict (user_id) do update set
             goal = excluded.goal,
             jenjang = excluded.jenjang,
             interests = excluded.interests,
             target_labels = excluded.target_labels,
             daily_minutes = excluded.daily_minutes,
             updated_at = now()
           returning goal, jenjang, interests, target_labels, daily_minutes, onboarding_completed_at"#,
        ctx.user_id,
        req.goal,
        req.jenjang,
        interests,
        target_labels,
        req.daily_minutes,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /me/learning-profile — `null` when the user never went through
// onboarding (every account created before this shipped).
pub async fn get(pool: &PgPool, ctx: &AuthContext) -> Result<Option<LearningProfile>, AppError> {
    let row = sqlx::query_as!(
        LearningProfile,
        r#"select goal, jenjang, interests, target_labels, daily_minutes, onboarding_completed_at
           from user_learning_profiles where user_id = $1"#,
        ctx.user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// POST /me/learning-profile/complete-onboarding — dismisses the
// dashboard walkthrough. Creates the row if the user somehow reached
// the dashboard without one (an account that predates onboarding).
pub async fn complete_onboarding(pool: &PgPool, ctx: &AuthContext) -> Result<LearningProfile, AppError> {
    let row = sqlx::query_as!(
        LearningProfile,
        r#"insert into user_learning_profiles (user_id, onboarding_completed_at)
           values ($1, now())
           on conflict (user_id) do update set onboarding_completed_at = now(), updated_at = now()
           returning goal, jenjang, interests, target_labels, daily_minutes, onboarding_completed_at"#,
        ctx.user_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}
