use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::Config;

const SUGGESTED_QUESTIONS_PER_CONCEPT: i64 = 3;

// --- Pure math (ADR-0003, SM-2 variant) — ported from frss.ts. ---

pub const MIN_EASE_FACTOR: f64 = 1.3;
pub const MAX_EASE_FACTOR: f64 = 2.8;
pub const MIN_INTERVAL_DAYS: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewResult {
    Recalled,
    Partial,
    Forgot,
}

impl ReviewResult {
    fn as_str(self) -> &'static str {
        match self {
            ReviewResult::Recalled => "recalled",
            ReviewResult::Partial => "partial",
            ReviewResult::Forgot => "forgot",
        }
    }
}

pub fn classify(correctness: f64, recalled_threshold: f64, partial_threshold: f64) -> ReviewResult {
    if correctness >= recalled_threshold {
        ReviewResult::Recalled
    } else if correctness >= partial_threshold {
        ReviewResult::Partial
    } else {
        ReviewResult::Forgot
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FrssState {
    pub interval_days: f64,
    pub ease_factor: f64,
}

pub fn default_frss_state() -> FrssState {
    FrssState { interval_days: MIN_INTERVAL_DAYS, ease_factor: 2.5 }
}

pub fn apply(state: FrssState, result: ReviewResult) -> FrssState {
    let (interval_days, ease_factor) = match result {
        ReviewResult::Recalled => (state.interval_days * state.ease_factor, (state.ease_factor + 0.1).min(MAX_EASE_FACTOR)),
        ReviewResult::Partial => (state.interval_days * 1.2, (state.ease_factor - 0.15).max(MIN_EASE_FACTOR)),
        ReviewResult::Forgot => (MIN_INTERVAL_DAYS, (state.ease_factor - 0.3).max(MIN_EASE_FACTOR)),
    };
    FrssState { interval_days: interval_days.max(MIN_INTERVAL_DAYS), ease_factor }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_boundaries_are_inclusive() {
        assert_eq!(classify(0.8, 0.8, 0.4), ReviewResult::Recalled);
        assert_eq!(classify(0.4, 0.8, 0.4), ReviewResult::Partial);
        assert_eq!(classify(0.39, 0.8, 0.4), ReviewResult::Forgot);
    }

    #[test]
    fn five_cycle_alternating_review_table() {
        // recalled, recalled, forgot, recalled, partial — starting from
        // the default state (interval=1.0, ease=2.5), verifying each
        // step matches apply()'s own per-outcome formula exactly.
        let mut state = default_frss_state();

        state = apply(state, ReviewResult::Recalled); // interval 1*2.5=2.5, ease 2.6
        assert!((state.interval_days - 2.5).abs() < 1e-9);
        assert!((state.ease_factor - 2.6).abs() < 1e-9);

        state = apply(state, ReviewResult::Recalled); // interval 2.5*2.6=6.5, ease 2.7
        assert!((state.interval_days - 6.5).abs() < 1e-9);
        assert!((state.ease_factor - 2.7).abs() < 1e-9);

        state = apply(state, ReviewResult::Forgot); // interval resets to 1.0, ease 2.4
        assert!((state.interval_days - 1.0).abs() < 1e-9);
        assert!((state.ease_factor - 2.4).abs() < 1e-9);

        state = apply(state, ReviewResult::Recalled); // interval 1*2.4=2.4, ease 2.5
        assert!((state.interval_days - 2.4).abs() < 1e-9);
        assert!((state.ease_factor - 2.5).abs() < 1e-9);

        state = apply(state, ReviewResult::Partial); // interval 2.4*1.2=2.88, ease 2.35
        assert!((state.interval_days - 2.88).abs() < 1e-9);
        assert!((state.ease_factor - 2.35).abs() < 1e-9);
    }

    #[test]
    fn ease_factor_clamps_at_bounds() {
        let mut state = FrssState { interval_days: 1.0, ease_factor: MAX_EASE_FACTOR };
        state = apply(state, ReviewResult::Recalled);
        assert_eq!(state.ease_factor, MAX_EASE_FACTOR);

        let mut state = FrssState { interval_days: 1.0, ease_factor: MIN_EASE_FACTOR };
        state = apply(state, ReviewResult::Forgot);
        assert_eq!(state.ease_factor, MIN_EASE_FACTOR);
    }

    #[test]
    fn interval_never_drops_below_the_floor() {
        let state = FrssState { interval_days: 0.5, ease_factor: 2.5 };
        let next = apply(state, ReviewResult::Forgot);
        assert_eq!(next.interval_days, MIN_INTERVAL_DAYS);
    }
}

async fn find_state(pool: &PgPool, user_id: Uuid, concept_id: Uuid) -> Result<Option<FrssState>, AppError> {
    struct Row {
        interval_days: f64,
        ease_factor: f64,
    }
    let row = sqlx::query_as!(Row, r#"select interval_days, ease_factor from frss_schedule where user_id = $1 and concept_id = $2"#, user_id, concept_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| FrssState { interval_days: r.interval_days, ease_factor: r.ease_factor }))
}

fn round_to_nearest_hour(dt: DateTime<Utc>) -> DateTime<Utc> {
    let rounded_secs = (dt.timestamp() as f64 / 3600.0).round() as i64 * 3600;
    DateTime::from_timestamp(rounded_secs, 0).unwrap_or(dt)
}

async fn upsert_schedule(pool: &PgPool, user_id: Uuid, concept_id: Uuid, interval_days: f64, ease_factor: f64, due_at: DateTime<Utc>, last_result: &str) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into frss_schedule (user_id, concept_id, interval_days, ease_factor, due_at, last_result) values ($1, $2, $3, $4, $5, $6)
           on conflict (user_id, concept_id) do update
             set interval_days = excluded.interval_days, ease_factor = excluded.ease_factor, due_at = excluded.due_at, last_result = excluded.last_result"#,
        user_id,
        concept_id,
        interval_days,
        ease_factor,
        due_at,
        last_result,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// P22-002-adjacent note preserved from frss_service.ts: ADR-0003 says
// FRSS should fire from a dedicated review flow, not a regular
// assessment submit — there's no such flow, so submit_attempt/
// check_answer treat every regular graded answer as the review signal.
// Deliberate, documented bug-for-bug port target, not "fixed" here.
pub async fn record_review(pool: &PgPool, config: &Config, user_id: Uuid, concept_id: Uuid, correctness: f64) -> Result<(), AppError> {
    let state = find_state(pool, user_id, concept_id).await?.unwrap_or_else(default_frss_state);
    let result = classify(correctness, config.frss_recalled_threshold, config.frss_partial_threshold);
    let new_state = apply(state, result);
    let now = Utc::now();
    let due_at = round_to_nearest_hour(now + chrono::Duration::seconds((new_state.interval_days * 86400.0).round() as i64));
    upsert_schedule(pool, user_id, concept_id, new_state.interval_days, new_state.ease_factor, due_at, result.as_str()).await
}

pub struct ReviewQueueRow {
    pub concept_id: Uuid,
    pub concept_name: String,
    pub due_at: DateTime<Utc>,
}

// Port of frss_repository.ts's findDue.
pub async fn find_due(pool: &PgPool, user_id: Uuid, now: DateTime<Utc>, gap_cutoff: DateTime<Utc>, limit: i64) -> Result<Vec<ReviewQueueRow>, AppError> {
    let rows = sqlx::query_as!(
        ReviewQueueRow,
        r#"select fs.concept_id as "concept_id!", c.name as "concept_name!", fs.due_at as "due_at!"
           from frss_schedule fs
           inner join concepts c on c.id = fs.concept_id
           left join masteries m on m.user_id = fs.user_id and m.concept_id = fs.concept_id
           where fs.user_id = $1
             and fs.due_at <= $2
             and (m.last_reviewed_at is null or m.last_reviewed_at <= $3)
           order by fs.due_at asc, coalesce(m.score, 0) asc
           limit $4"#,
        user_id,
        now,
        gap_cutoff,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn find_last_answered_question_type(pool: &PgPool, user_id: Uuid, concept_id: Uuid) -> Result<Option<String>, AppError> {
    let row = sqlx::query_scalar!(
        r#"SELECT q.type
           FROM learning_events le
           JOIN question_concepts qc ON qc.question_id = le.entity_id
           JOIN questions q ON q.id = le.entity_id
           WHERE le.user_id = $1
             AND qc.concept_id = $2
             AND le.event_type = 'question_answered'
             AND le.entity_type = 'question'
           ORDER BY le.created_at DESC
           LIMIT 1"#,
        user_id,
        concept_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// P5-001 (§3.4 Retrieval Variation) — surfaces a different question
// type from the one the learner was just reviewed with, filling any
// remaining slots with the rest.
pub async fn find_suggested_question_ids(pool: &PgPool, user_id: Uuid, concept_id: Uuid, limit: i64) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query!(
        r#"select id, type from questions
           inner join question_concepts qc on qc.question_id = questions.id
           where qc.concept_id = $1 and status = 'published'
           order by id asc"#,
        concept_id,
    )
    .fetch_all(pool)
    .await?;

    let distinct_types: std::collections::HashSet<&str> = rows.iter().map(|r| r.r#type.as_str()).collect();
    if distinct_types.len() <= 1 {
        return Ok(rows.into_iter().take(limit as usize).map(|r| r.id).collect());
    }

    let Some(last_type) = find_last_answered_question_type(pool, user_id, concept_id).await? else {
        return Ok(rows.into_iter().take(limit as usize).map(|r| r.id).collect());
    };

    let (different, same): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| r.r#type != last_type);
    Ok(different.into_iter().chain(same).take(limit as usize).map(|r| r.id).collect())
}

#[derive(Debug, serde::Serialize)]
pub struct ReviewQueueItem {
    pub concept_id: Uuid,
    pub concept_name: String,
    pub due_at: DateTime<Utc>,
    pub suggested_question_ids: Vec<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct ReviewQueueResponse {
    pub items: Vec<ReviewQueueItem>,
}

// GET /review-queue?limit= — requestedLimit is capped at
// config.review_queue_default_limit (also the default when omitted), a
// caller can ask for fewer, never more (ADR-0003's anti-punitive
// "batch, bukan seluruh backlog" rule).
pub async fn get_review_queue(pool: &PgPool, config: &Config, user_id: Uuid, requested_limit: Option<i64>) -> Result<ReviewQueueResponse, AppError> {
    let cap = config.review_queue_default_limit;
    let limit = requested_limit.map(|l| l.clamp(1, cap)).unwrap_or(cap);

    let now = Utc::now();
    let gap_cutoff = now - chrono::Duration::hours(config.review_queue_min_gap_hours);

    let due = find_due(pool, user_id, now, gap_cutoff, limit).await?;

    let mut items = Vec::new();
    for row in due {
        let suggested_question_ids = find_suggested_question_ids(pool, user_id, row.concept_id, SUGGESTED_QUESTIONS_PER_CONCEPT).await?;
        items.push(ReviewQueueItem { concept_id: row.concept_id, concept_name: row.concept_name, due_at: row.due_at, suggested_question_ids });
    }

    Ok(ReviewQueueResponse { items })
}
