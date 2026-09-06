use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::Config;

// ADR-0012 — Rescue Mode. "3 kali gagal" is operationalized as the
// caller's last N answered questions for this concept (newest-first)
// all being incorrect.

#[derive(Debug, serde::Serialize)]
pub struct RescueStatusResponse {
    pub concept_id: Uuid,
    pub triggered: bool,
    pub consecutive_failures: i64,
    pub suggested_item_ids: Vec<Uuid>,
}

const MAX_SUGGESTED_ITEMS: i64 = 3;

struct RecentEvent {
    correct: Option<bool>,
}

async fn find_recent_events_for_concept(pool: &PgPool, user_id: Uuid, concept_id: Uuid, limit: i64) -> Result<Vec<RecentEvent>, AppError> {
    let rows = sqlx::query!(
        r#"SELECT (le.payload->>'correct')::boolean AS correct
           FROM learning_events le
           JOIN question_concepts qc ON qc.question_id = le.entity_id
           WHERE le.user_id = $1
             AND qc.concept_id = $2
             AND le.event_type = 'question_answered'
             AND le.entity_type = 'question'
           ORDER BY le.created_at DESC
           LIMIT $3"#,
        user_id,
        concept_id,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| RecentEvent { correct: r.correct }).collect())
}

async fn find_published_items_for_concept(pool: &PgPool, concept_id: Uuid, limit: i64) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"select i.id from module_item_concepts ic inner join module_items i on i.id = ic.item_id
           where ic.concept_id = $1 and i.node_type = 'item' and i.status = 'published' limit $2"#,
        concept_id,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// Counts from the newest event backward until the first correct answer
// (or non-boolean/ungraded answer) breaks the streak — reported even
// when `triggered` is false so the frontend can show "1 more wrong
// answer and we'll suggest slowing down" rather than a bare boolean.
fn count_leading_failures(recent_newest_first: &[RecentEvent]) -> i64 {
    let mut count = 0;
    for event in recent_newest_first {
        if event.correct != Some(false) {
            break;
        }
        count += 1;
    }
    count
}

pub async fn get_rescue_status(pool: &PgPool, config: &Config, user_id: Uuid, concept_id: Uuid) -> Result<RescueStatusResponse, AppError> {
    let threshold = config.rescue_mode_consecutive_failures;
    let recent = find_recent_events_for_concept(pool, user_id, concept_id, threshold).await?;

    let triggered = recent.len() as i64 >= threshold && recent.iter().all(|e| e.correct == Some(false));
    let suggested_items =
        if triggered { find_published_items_for_concept(pool, concept_id, MAX_SUGGESTED_ITEMS).await? } else { Vec::new() };

    Ok(RescueStatusResponse {
        concept_id,
        triggered,
        consecutive_failures: count_leading_failures(&recent),
        suggested_item_ids: suggested_items,
    })
}
