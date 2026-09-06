use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;

// Port of module_completion_service.ts (§37, ADR-0012 P21-002). Phase
// 31 rename: "module" here now maps to `module_items` (a real table,
// unlike the old "lessons" mapping that only existed because no
// separate modules table existed yet). An item only has something to
// gate when it has both embedded questions (accuracy) AND linked
// concepts (mastery); otherwise it reports itself as not applicable
// rather than a vacuous "completed: true".
async fn embedded_question_ids(pool: &PgPool, item_id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let blocks = crate::services::module_item::find_content_blocks(pool, item_id).await?;
    Ok(blocks
        .into_iter()
        .filter(|b| b.r#type == "question_embed")
        .filter_map(|b| b.data.get("question_id").and_then(|v| v.as_str()).and_then(|s| Uuid::parse_str(s).ok()))
        .collect())
}

async fn find_concept_ids_for_item(pool: &PgPool, item_id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(r#"select concept_id from module_item_concepts where item_id = $1"#, item_id).fetch_all(pool).await?;
    Ok(rows)
}

async fn find_override(pool: &PgPool, user_id: Uuid, item_id: Uuid) -> Result<bool, AppError> {
    let row = sqlx::query_scalar!(
        r#"select user_id from item_completion_overrides where user_id = $1 and item_id = $2"#,
        user_id,
        item_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

// DISTINCT ON (entity_id) latest 'question_answered' event per question,
// keyed by (payload->>'correct')::boolean — matches
// learning_event_repository.ts's findLatestByUserAndQuestions exactly,
// including unanswered questions being absent from the map (not false).
async fn find_latest_correctness(pool: &PgPool, user_id: Uuid, question_ids: &[Uuid]) -> Result<std::collections::HashMap<Uuid, bool>, AppError> {
    if question_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = sqlx::query!(
        r#"select distinct on (entity_id) entity_id as "entity_id!", (payload->>'correct')::boolean as correct
           from learning_events
           where user_id = $1 and event_type = 'question_answered' and entity_type = 'question' and entity_id = any($2)
           order by entity_id, created_at desc"#,
        user_id,
        question_ids,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.entity_id, r.correct == Some(true))).collect())
}

#[derive(Debug, serde::Serialize)]
pub struct ItemCompletionStatusResponse {
    pub item_id: Uuid,
    pub eligible_for_gate: bool,
    pub accuracy: Option<f64>,
    pub required_activities_completed: bool,
    pub minimum_mastery_reached: bool,
    pub completed: bool,
    pub skipped: bool,
    pub skip_credit_cost: i64,
}

// GET /module-items/{id}/completion-status
pub async fn get_completion_status(pool: &PgPool, config: &Config, user_id: Uuid, item_id: Uuid) -> Result<ItemCompletionStatusResponse, AppError> {
    let exists = sqlx::query_scalar!(r#"select id from module_items where id = $1"#, item_id).fetch_optional(pool).await?;
    if exists.is_none() {
        return Err(AppError::NotFound("module_item_not_found"));
    }

    let skipped = find_override(pool, user_id, item_id).await?;
    let question_ids = embedded_question_ids(pool, item_id).await?;
    let concept_ids = find_concept_ids_for_item(pool, item_id).await?;

    if question_ids.is_empty() || concept_ids.is_empty() {
        return Ok(ItemCompletionStatusResponse {
            item_id,
            eligible_for_gate: false,
            accuracy: None,
            required_activities_completed: false,
            minimum_mastery_reached: false,
            completed: skipped,
            skipped,
            skip_credit_cost: config.module_completion_skip_credit_cost,
        });
    }

    let latest_by_question = find_latest_correctness(pool, user_id, &question_ids).await?;
    let answered_count = latest_by_question.len();
    let correct_count = latest_by_question.values().filter(|v| **v).count();
    // Unanswered questions count as not-yet-correct — no partial credit
    // for what was skipped.
    let accuracy = (correct_count as f64 / question_ids.len() as f64) * 100.0;
    let required_activities_completed = answered_count == question_ids.len();

    let mut minimum_mastery_reached = true;
    for concept_id in &concept_ids {
        let score = sqlx::query_scalar!(r#"select score from masteries where user_id = $1 and concept_id = $2"#, user_id, concept_id).fetch_optional(pool).await?;
        if score.is_none_or(|s| s < config.weakness_score_threshold) {
            minimum_mastery_reached = false;
            break;
        }
    }

    let naturally_completed = accuracy >= config.module_completion_min_accuracy && required_activities_completed && minimum_mastery_reached;

    Ok(ItemCompletionStatusResponse {
        item_id,
        eligible_for_gate: true,
        accuracy: Some(accuracy.round()),
        required_activities_completed,
        minimum_mastery_reached,
        completed: naturally_completed || skipped,
        skipped,
        skip_credit_cost: config.module_completion_skip_credit_cost,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct SkipCompletionResponse {
    pub item_id: Uuid,
    pub credit_charged: i64,
}

// POST /module-items/{id}/skip-completion — reuses economy::charge as-is.
// Idempotent: a 2nd skip on an already-skipped item charges nothing
// more and just confirms the override.
pub async fn skip_completion(pool: &PgPool, config: &Config, user_id: Uuid, item_id: Uuid) -> Result<SkipCompletionResponse, AppError> {
    let exists = sqlx::query_scalar!(r#"select id from module_items where id = $1"#, item_id).fetch_optional(pool).await?;
    if exists.is_none() {
        return Err(AppError::NotFound("module_item_not_found"));
    }

    let already_skipped = find_override(pool, user_id, item_id).await?;
    if already_skipped {
        return Ok(SkipCompletionResponse { item_id, credit_charged: 0 });
    }

    let required = config.module_completion_skip_credit_cost;
    let balance = crate::services::economy::get_balance(pool, user_id).await?;
    if balance < required {
        return Err(AppError::InsufficientCredit { required, balance });
    }

    crate::services::economy::charge(pool, user_id, required, &format!("item_completion_skip:{item_id}")).await?;
    sqlx::query!(
        r#"insert into item_completion_overrides (user_id, item_id) values ($1, $2) on conflict do nothing"#,
        user_id,
        item_id,
    )
    .execute(pool)
    .await?;

    Ok(SkipCompletionResponse { item_id, credit_charged: required })
}
