use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::{question, streak, xp};

struct SkillAchievement {
    code: &'static str,
    name: &'static str,
    description: &'static str,
}

fn skill_achievement(skill_category: &str) -> Option<SkillAchievement> {
    match skill_category {
        "speaking" => Some(SkillAchievement { code: "speaking_star", name: "Speaking Star", description: "Earned significant XP practicing speaking." }),
        "listening" => Some(SkillAchievement { code: "listening_master", name: "Listening Master", description: "Earned significant XP practicing listening." }),
        "reading" => Some(SkillAchievement { code: "reading_explorer", name: "Reading Explorer", description: "Earned significant XP practicing reading." }),
        "writing" => Some(SkillAchievement { code: "writing_builder", name: "Writing Builder", description: "Earned significant XP practicing writing." }),
        _ => None,
    }
}
const SKILL_XP_THRESHOLD: i64 = 100;

struct ImprovementAchievement {
    code: &'static str,
    name: &'static str,
    description: &'static str,
    threshold: f64,
}

const IMPROVEMENT_ACHIEVEMENTS: [ImprovementAchievement; 3] = [
    ImprovementAchievement { code: "improver", name: "Improver", description: "A concept's mastery grew by 10+ points.", threshold: 10.0 },
    ImprovementAchievement { code: "weakness_destroyer", name: "Weakness Destroyer", description: "A concept's mastery grew by 20+ points.", threshold: 20.0 },
    ImprovementAchievement { code: "master_of_growth", name: "Master of Growth", description: "A concept's mastery grew by 40+ points.", threshold: 40.0 },
];

struct CatalogEntry {
    code: &'static str,
    category: &'static str,
    name: &'static str,
    description: &'static str,
    criteria: serde_json::Value,
}

fn catalog() -> Vec<CatalogEntry> {
    let mut entries = vec![
        CatalogEntry { code: "first_lesson", category: "learning", name: "First Lesson", description: "Completed your first learning activity.", criteria: serde_json::json!({"type": "first_activity"}) },
        CatalogEntry { code: "streak_7", category: "learning", name: "7-Day Streak", description: "Kept a 7-day learning streak going.", criteria: serde_json::json!({"type": "streak", "days": 7}) },
        CatalogEntry { code: "streak_30", category: "learning", name: "30-Day Streak", description: "Kept a 30-day learning streak going.", criteria: serde_json::json!({"type": "streak", "days": 30}) },
    ];
    for skill in ["speaking", "listening", "reading", "writing"] {
        let a = skill_achievement(skill).unwrap();
        entries.push(CatalogEntry { code: a.code, category: "skill", name: a.name, description: a.description, criteria: serde_json::json!({"type": "skill_xp", "threshold": SKILL_XP_THRESHOLD}) });
    }
    for a in &IMPROVEMENT_ACHIEVEMENTS {
        entries.push(CatalogEntry { code: a.code, category: "improvement", name: a.name, description: a.description, criteria: serde_json::json!({"type": "mastery_improvement", "threshold": a.threshold}) });
    }
    entries
}

async fn ensure_catalog(pool: &PgPool) -> Result<(), AppError> {
    for entry in catalog() {
        sqlx::query!(
            r#"insert into achievements (code, category, name, description, criteria) values ($1, $2, $3, $4, $5)
               on conflict (code) do nothing"#,
            entry.code,
            entry.category,
            entry.name,
            entry.description,
            entry.criteria,
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn find_earned_codes(pool: &PgPool, user_id: Uuid) -> Result<std::collections::HashSet<String>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"select a.code from user_achievements ua inner join achievements a on a.id = ua.achievement_id where ua.user_id = $1"#,
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

// Idempotent — awarding a code the user already has is a no-op.
async fn award(pool: &PgPool, user_id: Uuid, code: &str) -> Result<(), AppError> {
    let achievement_id = sqlx::query_scalar!(r#"select id from achievements where code = $1"#, code)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("unknown achievement code: {code} — is it in the catalog?")))?;
    sqlx::query!(
        r#"insert into user_achievements (user_id, achievement_id) values ($1, $2) on conflict do nothing"#,
        user_id,
        achievement_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn find_baseline(pool: &PgPool, user_id: Uuid, concept_id: Uuid) -> Result<Option<f64>, AppError> {
    let score = sqlx::query_scalar!(
        r#"select baseline_score from user_concept_mastery_baselines where user_id = $1 and concept_id = $2"#,
        user_id,
        concept_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(score)
}

async fn record_baseline_if_absent(pool: &PgPool, user_id: Uuid, concept_id: Uuid, score: f64) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into user_concept_mastery_baselines (user_id, concept_id, baseline_score) values ($1, $2, $3)
           on conflict (user_id, concept_id) do nothing"#,
        user_id,
        concept_id,
        score,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn find_mastery_score(pool: &PgPool, user_id: Uuid, concept_id: Uuid) -> Result<Option<f64>, AppError> {
    let score = sqlx::query_scalar!(r#"select score from masteries where user_id = $1 and concept_id = $2"#, user_id, concept_id)
        .fetch_optional(pool)
        .await?;
    Ok(score)
}

#[derive(Default)]
pub struct ActivityContext {
    pub skill_category: Option<String>,
    pub question_ids: Vec<Uuid>,
}

// Called from the same hook sites as xp::award_xp/streak::record_activity
// (deferred — see R6 ticket notes). 3 categories: Learning (activity/
// streak milestones), Skill (per skill_category XP threshold),
// Improvement (mastery growth vs a fixed baseline).
pub async fn check_and_award(pool: &PgPool, user_id: Uuid, context: &ActivityContext) -> Result<(), AppError> {
    ensure_catalog(pool).await?;
    let mut earned = find_earned_codes(pool, user_id).await?;

    check_learning_achievements(pool, user_id, &earned).await?;
    if let Some(skill_category) = &context.skill_category {
        check_skill_achievement(pool, user_id, skill_category, &earned).await?;
    }
    if !context.question_ids.is_empty() {
        check_improvement_achievements(pool, user_id, &context.question_ids, &mut earned).await?;
    }
    Ok(())
}

async fn check_learning_achievements(pool: &PgPool, user_id: Uuid, earned: &std::collections::HashSet<String>) -> Result<(), AppError> {
    if !earned.contains("first_lesson") {
        let total = xp::get_total(pool, user_id).await?;
        if total > 0 {
            award(pool, user_id, "first_lesson").await?;
        }
    }

    let current_streak = streak::find_current_streak(pool, user_id).await?;
    if current_streak >= 7 && !earned.contains("streak_7") {
        award(pool, user_id, "streak_7").await?;
    }
    if current_streak >= 30 && !earned.contains("streak_30") {
        award(pool, user_id, "streak_30").await?;
    }
    Ok(())
}

async fn check_skill_achievement(pool: &PgPool, user_id: Uuid, skill_category: &str, earned: &std::collections::HashSet<String>) -> Result<(), AppError> {
    let Some(achievement) = skill_achievement(skill_category) else { return Ok(()) };
    if earned.contains(achievement.code) {
        return Ok(());
    }
    let total = xp::get_total_by_skill_category(pool, user_id, skill_category).await?;
    if total >= SKILL_XP_THRESHOLD {
        award(pool, user_id, achievement.code).await?;
    }
    Ok(())
}

async fn check_improvement_achievements(pool: &PgPool, user_id: Uuid, question_ids: &[Uuid], earned: &mut std::collections::HashSet<String>) -> Result<(), AppError> {
    let concept_ids = question::find_concept_ids_for_questions(pool, question_ids).await?;

    for concept_id in concept_ids {
        let Some(score) = find_mastery_score(pool, user_id, concept_id).await? else { continue };

        let Some(baseline) = find_baseline(pool, user_id, concept_id).await? else {
            record_baseline_if_absent(pool, user_id, concept_id, score).await?;
            continue;
        };

        let delta = score - baseline;
        for improvement in &IMPROVEMENT_ACHIEVEMENTS {
            if delta >= improvement.threshold && !earned.contains(improvement.code) {
                award(pool, user_id, improvement.code).await?;
                earned.insert(improvement.code.to_string());
            }
        }
    }
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct AchievementSummary {
    pub code: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub earned_at: DateTime<Utc>,
}

// GET /me/achievements
pub async fn get_achievements(pool: &PgPool, user_id: Uuid) -> Result<Vec<AchievementSummary>, AppError> {
    let rows = sqlx::query_as!(
        AchievementSummary,
        r#"select a.code, a.name, a.description, a.category, ua.earned_at
           from user_achievements ua inner join achievements a on a.id = ua.achievement_id
           where ua.user_id = $1"#,
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
