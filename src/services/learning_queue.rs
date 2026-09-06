use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::{concept, frss};
use crate::Config;
use sqlx::PgPool;

const SUGGESTED_QUESTIONS_PER_CONCEPT: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Critical,
    Due,
    Weak,
}

fn priority_rank(p: Priority) -> u8 {
    match p {
        Priority::Critical => 0,
        Priority::Due => 1,
        Priority::Weak => 2,
    }
}

#[derive(Debug, serde::Serialize)]
pub struct LearningQueueItem {
    pub concept_id: Uuid,
    pub concept_name: String,
    pub priority: Priority,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub suggested_question_ids: Vec<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by_concept_id: Option<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct LearningQueueResponse {
    pub items: Vec<LearningQueueItem>,
}

struct MergedEntry {
    concept_id: Uuid,
    concept_name: String,
    due_at: Option<DateTime<Utc>>,
    score: Option<f64>,
    priority: Priority,
    blocked_by_concept_id: Option<Uuid>,
}

struct WeakConceptRow {
    concept_id: Uuid,
    concept_name: String,
    score: f64,
}

// Port of mastery_repository.ts's findWeak — concepts with a
// confident-but-low mastery score, weakest first.
async fn find_weak(pool: &PgPool, user_id: Uuid, score_threshold: f64, confidence_threshold: f64, limit: i64) -> Result<Vec<WeakConceptRow>, AppError> {
    let rows = sqlx::query_as!(
        WeakConceptRow,
        r#"select m.concept_id as "concept_id!", c.name as "concept_name!", m.score as "score!"
           from masteries m inner join concepts c on c.id = m.concept_id
           where m.user_id = $1 and m.confidence >= $2 and m.score < $3
           order by m.score asc
           limit $4"#,
        user_id,
        confidence_threshold,
        score_threshold,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn find_mastery_score(pool: &PgPool, user_id: Uuid, concept_id: Uuid) -> Result<Option<f64>, AppError> {
    let score = sqlx::query_scalar!(r#"select score from masteries where user_id = $1 and concept_id = $2"#, user_id, concept_id)
        .fetch_optional(pool)
        .await?;
    Ok(score)
}

// GET /learning-queue?limit= — merges 2 signals into one prioritized
// list: concepts due for FRSS review and concepts with a confident-but-
// weak mastery score that isn't due yet. Same limit cap/default rule as
// GET /review-queue.
pub async fn get_learning_queue(pool: &PgPool, config: &Config, user_id: Uuid, requested_limit: Option<i64>) -> Result<LearningQueueResponse, AppError> {
    let cap = config.review_queue_default_limit;
    let limit = requested_limit.map(|l| l.clamp(1, cap)).unwrap_or(cap);

    let now = Utc::now();
    let gap_cutoff = now - chrono::Duration::hours(config.review_queue_min_gap_hours);

    let due = frss::find_due(pool, user_id, now, gap_cutoff, limit).await?;
    let weak = find_weak(pool, user_id, config.weakness_score_threshold, config.mastery_confidence_threshold, limit).await?;

    let mut by_concept_id: std::collections::HashMap<Uuid, MergedEntry> = std::collections::HashMap::new();
    for row in due {
        by_concept_id.insert(
            row.concept_id,
            MergedEntry { concept_id: row.concept_id, concept_name: row.concept_name, due_at: Some(row.due_at), score: None, priority: Priority::Due, blocked_by_concept_id: None },
        );
    }
    for row in weak {
        if let Some(existing) = by_concept_id.get_mut(&row.concept_id) {
            existing.score = Some(row.score);
            existing.priority = Priority::Critical;
        } else {
            by_concept_id.insert(
                row.concept_id,
                MergedEntry { concept_id: row.concept_id, concept_name: row.concept_name, due_at: None, score: Some(row.score), priority: Priority::Weak, blocked_by_concept_id: None },
            );
        }
    }

    // P5-002 — for each weak/critical candidate, check its direct
    // prerequisites. A prerequisite that is itself weak or has no
    // mastery data yet gets escalated to critical (or added fresh), and
    // the origin item is marked blocked_by_concept_id. Iterates a
    // snapshot taken before this pass mutates the map.
    let escalation_candidates: Vec<Uuid> = by_concept_id
        .values()
        .filter(|e| e.priority == Priority::Weak || e.priority == Priority::Critical)
        .map(|e| e.concept_id)
        .collect();

    for concept_id in escalation_candidates {
        let prerequisites = concept::find_prerequisites(pool, concept_id).await?;
        for prereq in prerequisites {
            let prereq_score = find_mastery_score(pool, user_id, prereq.concept_id).await?;
            let is_weak_or_unknown = prereq_score.map(|s| s < config.weakness_score_threshold).unwrap_or(true);
            if !is_weak_or_unknown {
                continue;
            }

            if let Some(entry) = by_concept_id.get_mut(&concept_id) {
                entry.blocked_by_concept_id = Some(prereq.concept_id);
            }
            if let Some(existing_prereq_entry) = by_concept_id.get_mut(&prereq.concept_id) {
                existing_prereq_entry.priority = Priority::Critical;
            } else {
                by_concept_id.insert(
                    prereq.concept_id,
                    MergedEntry {
                        concept_id: prereq.concept_id,
                        concept_name: prereq.name,
                        due_at: None,
                        score: prereq_score,
                        priority: Priority::Critical,
                        blocked_by_concept_id: None,
                    },
                );
            }
            break;
        }
    }

    let mut merged: Vec<MergedEntry> = by_concept_id.into_values().collect();
    merged.sort_by(|a, b| {
        let rank_diff = priority_rank(a.priority).cmp(&priority_rank(b.priority));
        if rank_diff != std::cmp::Ordering::Equal {
            return rank_diff;
        }
        // Within "due"/"critical": soonest due first. Within "weak":
        // weakest score first. Never mix the 2 comparisons across
        // buckets.
        match (a.due_at, b.due_at) {
            (Some(a_due), Some(b_due)) => return a_due.cmp(&b_due),
            _ => {}
        }
        match (a.score, b.score) {
            (Some(a_score), Some(b_score)) => a_score.partial_cmp(&b_score).unwrap_or(std::cmp::Ordering::Equal),
            _ => std::cmp::Ordering::Equal,
        }
    });

    let limited: Vec<MergedEntry> = merged.into_iter().take(limit as usize).collect();

    let mut items = Vec::new();
    for entry in limited {
        let suggested_question_ids = frss::find_suggested_question_ids(pool, user_id, entry.concept_id, SUGGESTED_QUESTIONS_PER_CONCEPT).await?;
        items.push(LearningQueueItem {
            concept_id: entry.concept_id,
            concept_name: entry.concept_name,
            priority: entry.priority,
            due_at: entry.due_at,
            score: entry.score,
            suggested_question_ids,
            blocked_by_concept_id: entry.blocked_by_concept_id,
        });
    }

    Ok(LearningQueueResponse { items })
}
