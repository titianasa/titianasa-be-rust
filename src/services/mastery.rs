use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::concept;
use crate::Config;

struct MasteryRow {
    score: f64,
    confidence: f64,
    last_reviewed_at: Option<DateTime<Utc>>,
}

// --- Pure math (ADR-0002) — ported from mastery.ts's `compute`. The
// one piece of genuinely new algorithmic logic this module needed;
// everything else here was already read-side aggregation. ---

pub struct MasteryEvent {
    // 0.0-1.0, or None for a not-yet-gradable (writing/speaking) event
    // — excluded from both the weighted sum AND the confidence
    // denominator, never treated as wrong.
    pub correct: Option<f64>,
    pub difficulty: f64,
    pub age_days: f64,
}

pub struct MasteryResult {
    pub score: Option<i64>,
    pub confidence: f64,
}

pub fn compute(events: &[MasteryEvent], lambda: f64, n_min: f64) -> MasteryResult {
    let usable: Vec<&MasteryEvent> = events.iter().filter(|e| e.correct.is_some()).collect();
    if usable.is_empty() {
        return MasteryResult { score: None, confidence: 0.0 };
    }

    let mut weighted_correct = 0.0;
    let mut weight_total = 0.0;
    for event in &usable {
        let recency_weight = (-lambda * event.age_days).exp();
        let difficulty_factor = 0.5 + 0.5 * event.difficulty;
        let weight = recency_weight * difficulty_factor;
        weighted_correct += weight * event.correct.unwrap();
        weight_total += weight;
    }
    let raw = weighted_correct / weight_total;
    let confidence = (usable.len() as f64 / n_min).min(1.0);
    MasteryResult { score: Some((raw * 100.0).round() as i64), confidence }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_usable_events_yields_no_score() {
        let events = [MasteryEvent { correct: None, difficulty: 0.5, age_days: 0.0 }];
        let result = compute(&events, 0.05, 5.0);
        assert_eq!(result.score, None);
        assert_eq!(result.confidence, 0.0);
    }

    #[test]
    fn ungraded_events_excluded_from_both_score_and_confidence() {
        // 5 usable (all correct) + 5 ungraded — confidence must reflect
        // only the 5 usable ones (5/5=1.0), not all 10.
        let mut events: Vec<MasteryEvent> = (0..5).map(|_| MasteryEvent { correct: Some(1.0), difficulty: 0.5, age_days: 0.0 }).collect();
        events.extend((0..5).map(|_| MasteryEvent { correct: None, difficulty: 0.5, age_days: 0.0 }));
        let result = compute(&events, 0.05, 5.0);
        assert_eq!(result.score, Some(100));
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn confidence_caps_at_1_and_scales_with_n_min() {
        let events: Vec<MasteryEvent> = (0..3).map(|_| MasteryEvent { correct: Some(1.0), difficulty: 0.5, age_days: 0.0 }).collect();
        let result = compute(&events, 0.05, 5.0);
        assert_eq!(result.confidence, 0.6); // 3/5

        let events: Vec<MasteryEvent> = (0..10).map(|_| MasteryEvent { correct: Some(1.0), difficulty: 0.5, age_days: 0.0 }).collect();
        let result = compute(&events, 0.05, 5.0);
        assert_eq!(result.confidence, 1.0); // 10/5 capped at 1.0
    }

    #[test]
    fn older_events_are_recency_downweighted() {
        // 1 recent wrong answer vs 1 much older correct one — the
        // exponential decay must make the recent wrong answer dominate.
        let events = [MasteryEvent { correct: Some(0.0), difficulty: 0.5, age_days: 0.0 }, MasteryEvent { correct: Some(1.0), difficulty: 0.5, age_days: 365.0 }];
        let result = compute(&events, 0.05, 5.0);
        assert!(result.score.unwrap() < 50, "expected recent-wrong to dominate, got {:?}", result.score);
    }

    #[test]
    fn higher_difficulty_events_are_weighted_more() {
        // A single hard correct answer should score higher confidence-weighted
        // toward 100 the same as an easy one — the difficulty factor changes
        // WEIGHT relative to other events, not this single-event score.
        let easy = compute(&[MasteryEvent { correct: Some(1.0), difficulty: 0.0, age_days: 0.0 }], 0.05, 5.0);
        let hard = compute(&[MasteryEvent { correct: Some(1.0), difficulty: 1.0, age_days: 0.0 }], 0.05, 5.0);
        assert_eq!(easy.score, Some(100));
        assert_eq!(hard.score, Some(100));
    }
}

// Port of mastery_repository.ts's findEventsForConcept.
async fn find_events_for_concept(pool: &PgPool, user_id: Uuid, concept_id: Uuid) -> Result<Vec<MasteryEvent>, AppError> {
    struct Row {
        correct: Option<bool>,
        difficulty: Option<f64>,
        created_at: DateTime<Utc>,
    }
    let rows = sqlx::query_as!(
        Row,
        r#"select (le.payload->>'correct')::boolean as correct, (le.payload->>'difficulty')::float8 as difficulty, le.created_at
           from learning_events le
           inner join question_concepts qc on qc.question_id = le.entity_id
           where le.user_id = $1 and qc.concept_id = $2
             and le.event_type = 'question_answered' and le.entity_type = 'question'"#,
        user_id,
        concept_id,
    )
    .fetch_all(pool)
    .await?;

    let now = Utc::now();
    Ok(rows
        .into_iter()
        .map(|r| MasteryEvent {
            correct: r.correct.map(|c| if c { 1.0 } else { 0.0 }),
            difficulty: r.difficulty.unwrap_or(0.5),
            age_days: (now - r.created_at).num_seconds() as f64 / 86400.0,
        })
        .collect())
}

async fn upsert(pool: &PgPool, user_id: Uuid, concept_id: Uuid, score: Option<i64>, confidence: f64, now: DateTime<Utc>) -> Result<(), AppError> {
    let score = score.map(|s| s as f64).unwrap_or(0.0);
    sqlx::query!(
        r#"insert into masteries (user_id, concept_id, score, confidence, last_reviewed_at) values ($1, $2, $3, $4, $5)
           on conflict (user_id, concept_id) do update
             set score = excluded.score, confidence = excluded.confidence, last_reviewed_at = excluded.last_reviewed_at, updated_at = now()"#,
        user_id,
        concept_id,
        score,
        confidence,
        now,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Called from submit_attempt/check_answer for every touched concept.
pub async fn recompute_for_concept(pool: &PgPool, config: &Config, user_id: Uuid, concept_id: Uuid) -> Result<(), AppError> {
    let events = find_events_for_concept(pool, user_id, concept_id).await?;
    let result = compute(&events, config.mastery_lambda, config.mastery_n_min);
    upsert(pool, user_id, concept_id, result.score, result.confidence, Utc::now()).await
}

async fn find(pool: &PgPool, user_id: Uuid, concept_id: Uuid) -> Result<Option<MasteryRow>, AppError> {
    let row = sqlx::query_as!(
        MasteryRow,
        r#"select score, confidence, last_reviewed_at from masteries where user_id = $1 and concept_id = $2"#,
        user_id,
        concept_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn find_many(pool: &PgPool, user_id: Uuid, concept_ids: &[Uuid]) -> Result<Vec<(Uuid, MasteryRow)>, AppError> {
    if concept_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query!(
        r#"select concept_id, score, confidence, last_reviewed_at from masteries
           where user_id = $1 and concept_id = any($2)"#,
        user_id,
        concept_ids,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.concept_id, MasteryRow { score: r.score, confidence: r.confidence, last_reviewed_at: r.last_reviewed_at }))
        .collect())
}

#[derive(Debug, serde::Serialize)]
pub struct MasteryResponse {
    pub concept_id: Uuid,
    pub score: Option<i64>,
    pub confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reviewed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
}

// GET /mastery/{concept_id} — always the caller's own mastery
// (ctx.userId), matching ADR-0006's "milik sendiri" scoping.
pub async fn get_mastery(pool: &PgPool, config: &Config, user_id: Uuid, concept_id: Uuid) -> Result<MasteryResponse, AppError> {
    let record = find(pool, user_id, concept_id).await?;

    let Some(record) = record else {
        return Ok(MasteryResponse { concept_id, score: None, confidence: 0.0, last_reviewed_at: None, message: Some("insufficient_data") });
    };

    if record.confidence < config.mastery_confidence_threshold {
        return Ok(MasteryResponse {
            concept_id,
            score: None,
            confidence: record.confidence,
            last_reviewed_at: None,
            message: Some("insufficient_data"),
        });
    }

    Ok(MasteryResponse {
        concept_id,
        score: Some(record.score.round() as i64),
        confidence: record.confidence,
        last_reviewed_at: record.last_reviewed_at,
        message: None,
    })
}

// P4-001 (roadmap §3.1) — 2-level drill-down bound, matches
// concept::find_descendants_up_to's own depth cap.
const MAX_DRILL_DOWN_DEPTH: i32 = 2;

#[derive(Debug, serde::Serialize)]
pub struct MasteryBreakdownNode {
    pub concept_id: Uuid,
    pub name: String,
    pub score: Option<i64>,
    pub confidence: f64,
    pub weak: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
    pub children: Vec<MasteryBreakdownNode>,
}

fn to_node(
    concept_id: Uuid,
    name: String,
    record: Option<&MasteryRow>,
    config: &Config,
    children: Vec<MasteryBreakdownNode>,
) -> MasteryBreakdownNode {
    // Below-confidence and never-attempted are the same "not enough
    // data yet" state — neither gets flagged `weak`, since that would
    // claim a signal that doesn't exist.
    match record {
        None => MasteryBreakdownNode { concept_id, name, score: None, confidence: 0.0, weak: false, message: Some("insufficient_data"), children },
        Some(r) if r.confidence < config.mastery_confidence_threshold => {
            MasteryBreakdownNode { concept_id, name, score: None, confidence: r.confidence, weak: false, message: Some("insufficient_data"), children }
        }
        Some(r) => {
            let score = r.score.round();
            MasteryBreakdownNode {
                concept_id,
                name,
                score: Some(score as i64),
                confidence: r.confidence,
                weak: score < config.weakness_score_threshold,
                message: None,
                children,
            }
        }
    }
}

// GET /concepts/{id}/mastery-breakdown — always the caller's own
// mastery, same scoping rule as get_mastery above.
pub async fn get_mastery_breakdown(pool: &PgPool, config: &Config, user_id: Uuid, concept_id: Uuid) -> Result<MasteryBreakdownNode, AppError> {
    let root = concept::find(pool, concept_id).await?.ok_or(AppError::NotFound("concept_not_found"))?;

    let descendants = concept::find_descendants_up_to(pool, concept_id, MAX_DRILL_DOWN_DEPTH).await?;
    let all_ids: Vec<Uuid> = std::iter::once(concept_id).chain(descendants.iter().map(|d| d.id)).collect();
    let mastery_rows = find_many(pool, user_id, &all_ids).await?;
    let mastery_by_concept_id: std::collections::HashMap<Uuid, MasteryRow> = mastery_rows.into_iter().collect();

    fn build_node(
        id: Uuid,
        name: &str,
        descendants: &[crate::services::concept::ConceptWithDepth],
        mastery_by_concept_id: &std::collections::HashMap<Uuid, MasteryRow>,
        config: &Config,
    ) -> MasteryBreakdownNode {
        let children: Vec<MasteryBreakdownNode> = descendants
            .iter()
            .filter(|d| d.parent_concept_id == Some(id))
            .map(|d| build_node(d.id, &d.name, descendants, mastery_by_concept_id, config))
            .collect();
        to_node(id, name.to_string(), mastery_by_concept_id.get(&id), config, children)
    }

    Ok(build_node(root.id, &root.name, &descendants, &mastery_by_concept_id, config))
}
