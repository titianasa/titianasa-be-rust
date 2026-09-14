// The gate before generating at scale: for domains that already have a
// gold plan (the ones Claude wrote), Gemini plans them again without
// seeing that gold plan, and the judge scores both blind — labels
// shuffled per task. The report says whether Gemini is good enough to
// be trusted on the other few thousand topics.

use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::plan::{BabDraft, DomainContext, TopicContext, TopicDraft};
use super::qa::{self, PairwiseResult, DIMENSIONS};
use super::validate::Severity;
use crate::errors::AppError;
use crate::services::job_queue::JobContext;

/// What the report calls "setara": Gemini's average at least this share
/// of the gold average, no dimension below `DIMENSION_FLOOR` of gold,
/// and blockers on at most `MAX_BLOCKER_RATE` of topics.
pub const AVERAGE_RATIO: f64 = 0.95;
pub const DIMENSION_FLOOR: f64 = 0.90;
pub const MAX_BLOCKER_RATE: f64 = 0.05;

async fn gold_for(pool: &PgPool, domain_folder_id: Uuid) -> Result<Vec<(Uuid, Vec<BabDraft>)>, AppError> {
    let output = sqlx::query_scalar!(r#"select output from generation_exemplars where kind = 'bab_plan' and domain_folder_id = $1 and source = 'claude'"#, domain_folder_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("gold_plan_not_found"))?;
    let topics = output.get("topik").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(topics
        .into_iter()
        .filter_map(|t| {
            let id = t.get("topic_id")?.as_str()?.parse::<Uuid>().ok()?;
            let bab: Vec<BabDraft> = serde_json::from_value(t.get("bab")?.clone()).ok()?;
            Some((id, bab))
        })
        .collect())
}

/// Scores the generated drafts of one benchmark task against gold.
pub async fn judge_task(jc: &JobContext, owner: Uuid, task_id: Uuid, domain: &DomainContext, drafts: &[TopicDraft]) -> Result<(), AppError> {
    let gold = gold_for(&jc.pool, domain.domain_folder_id).await?;
    let pairs: Vec<(&TopicContext, &[BabDraft], &[BabDraft])> = domain
        .targets()
        .into_iter()
        .filter_map(|t| {
            let generated = drafts.iter().find(|d| d.topic_id == t.topic_id)?;
            let (_, g) = gold.iter().find(|(id, _)| *id == t.topic_id)?;
            Some((t, generated.bab.as_slice(), g.as_slice()))
        })
        .collect();
    if pairs.is_empty() {
        return Err(AppError::UnprocessableEntity("benchmark_empty", "tidak ada topik yang bisa dibandingkan".to_string()));
    }
    // Per-task shuffle, derived from the task id so a rerun judges the same way.
    let generated_is_a = task_id.as_bytes()[0] % 2 == 0;
    let topics: Vec<&TopicContext> = pairs.iter().map(|(t, _, _)| *t).collect();
    let gen: Vec<&[BabDraft]> = pairs.iter().map(|(_, g, _)| *g).collect();
    let gold_side: Vec<&[BabDraft]> = pairs.iter().map(|(_, _, g)| *g).collect();
    let (a, b) = if generated_is_a { (gen, gold_side) } else { (gold_side, gen) };
    let prompt = qa::pairwise_prompt(domain, &topics, &a, &b);

    let resolved = crate::services::ai_settings::resolve(&jc.pool, &jc.config, "agent_qa").await?;
    let max_tokens = crate::services::ai_provider::resolve_max_tokens(&jc.pool, &resolved.model_id, 32_000).await;
    let request = crate::services::ai_provider::GenerationRequest {
        model: resolved.model_id.clone(),
        system_prompt: qa::system_prompt(),
        user_prompt: prompt,
        temperature: resolved.temperature.unwrap_or(0.1),
        max_tokens,
        image_url: None,
        json_mode: true,
        thinking_budget: Some(2048),
        allow_partial: false,
    };
    let (result, model) = crate::services::ai_provider::generate_with_fallback(jc.text_ai.as_ref(), &resolved, request).await;
    let resp = result.map_err(|e| AppError::Internal(anyhow::anyhow!("penilai benchmark gagal: {e}")))?;
    let tokens = resp.tokens_used.unwrap_or(0);
    crate::services::ai_task::insert_done(&jc.pool, Uuid::new_v4(), owner, "content_factory_benchmark", "vertex", &model, super::plan::PROMPT_VERSION, Some(tokens as i32)).await?;
    let order: Vec<Uuid> = topics.iter().map(|t| t.topic_id).collect();
    let results = qa::parse_pairwise(&resp.text, &order, generated_is_a).map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    sqlx::query!(
        r#"update generation_tasks set qa = $2, tokens_qa = tokens_qa + $3, updated_at = now() where id = $1"#,
        task_id,
        json!({"pairwise": results, "judge_model": model, "generated_label": if generated_is_a { "A" } else { "B" }}),
        tokens,
    )
    .execute(&jc.pool)
    .await?;
    sqlx::query!(r#"update generation_runs set tokens_used = tokens_used + $2 where id = (select run_id from generation_tasks where id = $1)"#, task_id, tokens).execute(&jc.pool).await?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct DimensionComparison {
    pub dimension: String,
    pub generated: f64,
    pub gold: f64,
    pub ratio: f64,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkReport {
    pub topics: usize,
    pub generated_average: f64,
    pub gold_average: f64,
    pub average_ratio: f64,
    pub dimensions: Vec<DimensionComparison>,
    pub preferred_generated: usize,
    pub preferred_gold: usize,
    pub ties: usize,
    pub generated_blocker_rate: f64,
    pub gold_blocker_rate: f64,
    pub passes_gate: bool,
    pub gate_failures: Vec<String>,
    pub results: Vec<PairwiseResult>,
}

pub fn report(results: Vec<PairwiseResult>) -> BenchmarkReport {
    let n = results.len().max(1) as f64;
    let avg = |f: &dyn Fn(&PairwiseResult) -> f64| results.iter().map(f).sum::<f64>() / n;
    let generated_average = avg(&|r| r.generated.average());
    let gold_average = avg(&|r| r.gold.average());
    let ratio = |g: f64, o: f64| if o > 0.0 { g / o } else { 1.0 };
    let dimensions: Vec<DimensionComparison> = DIMENSIONS
        .iter()
        .map(|(d, _)| {
            let generated = avg(&|r| r.generated.skor.get(*d).copied().unwrap_or(0.0));
            let gold = avg(&|r| r.gold.skor.get(*d).copied().unwrap_or(0.0));
            DimensionComparison { dimension: d.to_string(), generated, gold, ratio: ratio(generated, gold) }
        })
        .collect();
    let blocker = |s: &qa::TopicScore| s.isu.iter().any(|i| i.tingkat == Severity::Blocker);
    let generated_blocker_rate = results.iter().filter(|r| blocker(&r.generated)).count() as f64 / n;
    let gold_blocker_rate = results.iter().filter(|r| blocker(&r.gold)).count() as f64 / n;
    let average_ratio = ratio(generated_average, gold_average);

    let mut gate_failures = Vec::new();
    if results.is_empty() {
        gate_failures.push("belum ada topik yang dinilai".to_string());
    }
    if average_ratio < AVERAGE_RATIO {
        gate_failures.push(format!("rata-rata Gemini {:.0}% dari rata-rata acuan (syarat ≥ {:.0}%)", average_ratio * 100.0, AVERAGE_RATIO * 100.0));
    }
    for d in dimensions.iter().filter(|d| d.ratio < DIMENSION_FLOOR) {
        gate_failures.push(format!("dimensi {} {:.0}% dari acuan (syarat ≥ {:.0}%)", d.dimension, d.ratio * 100.0, DIMENSION_FLOOR * 100.0));
    }
    if generated_blocker_rate > MAX_BLOCKER_RATE {
        gate_failures.push(format!("isu blocker pada {:.0}% topik (syarat ≤ {:.0}%)", generated_blocker_rate * 100.0, MAX_BLOCKER_RATE * 100.0));
    }

    BenchmarkReport {
        topics: results.len(),
        generated_average,
        gold_average,
        average_ratio,
        preferred_generated: results.iter().filter(|r| r.preferred == "generated").count(),
        preferred_gold: results.iter().filter(|r| r.preferred == "gold").count(),
        ties: results.iter().filter(|r| r.preferred == "tie").count(),
        dimensions,
        generated_blocker_rate,
        gold_blocker_rate,
        passes_gate: gate_failures.is_empty(),
        gate_failures,
        results,
    }
}

pub async fn run_report(pool: &PgPool, run_id: Uuid) -> Result<BenchmarkReport, AppError> {
    let rows = sqlx::query_scalar!(r#"select qa from generation_tasks where run_id = $1 and status = 'benchmarked'"#, run_id).fetch_all(pool).await?;
    let results: Vec<PairwiseResult> = rows.into_iter().flatten().filter_map(|qa| qa.get("pairwise").cloned()).filter_map(|v| serde_json::from_value::<Vec<PairwiseResult>>(v).ok()).flatten().collect();
    Ok(report(results))
}

#[cfg(test)]
mod tests {
    use super::super::qa::TopicScore;
    use super::*;
    use std::collections::HashMap;

    fn scored(v: f64) -> TopicScore {
        TopicScore { topic_id: Uuid::nil(), skor: DIMENSIONS.iter().map(|(d, _)| (d.to_string(), v)).collect::<HashMap<_, _>>(), isu: vec![] }
    }

    #[test]
    fn the_gate_passes_only_when_close_to_gold() {
        let close = (0..10).map(|_| PairwiseResult { topic_id: Uuid::nil(), generated: scored(4.6), gold: scored(4.7), preferred: "tie".into(), reason: String::new() }).collect();
        assert!(report(close).passes_gate);
        let far = (0..10).map(|_| PairwiseResult { topic_id: Uuid::nil(), generated: scored(3.9), gold: scored(4.7), preferred: "gold".into(), reason: String::new() }).collect::<Vec<_>>();
        let r = report(far);
        assert!(!r.passes_gate);
        assert!(r.gate_failures.iter().any(|f| f.contains("rata-rata")));
        assert_eq!(r.preferred_gold, 10);
    }
}
