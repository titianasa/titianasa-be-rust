use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest};
use crate::services::{ad, ai_task, economy};

const PROVIDER: &str = "deepseek";
const GRAMMAR_EVALUATION_PROMPT_ID: &str = "grammar_evaluation_v1";

fn grammar_evaluation_prompt(text: &str) -> (String, String) {
    let system = r#"You are a strict English grammar checker. Given a sentence, respond with ONLY a JSON object of the exact shape {"errors":[{"span":[start,end],"issue":"snake_case_issue_code","suggestion":"string"}]} — no prose, no markdown fences. `span` is a [start,end] character offset pair into the input. If there are no errors, return {"errors":[]}."#.to_string();
    let user = format!("Sentence: {text}");
    (system, user)
}

#[derive(Debug, serde::Serialize)]
pub struct EvaluateResponse {
    pub ai_task_id: Uuid,
    pub status: &'static str,
    pub result: serde_json::Value,
    pub credit_charged: i64,
}

// POST /ai/evaluate — ADR-0004's mandatory flow (cost estimate -> credit
// check -> provider call -> output validation -> usage tracking) for the
// one MVP task type, grammar_evaluation. The balance check and the
// actual charge are deliberately 2 separate DB round-trips, not 1
// transaction spanning the (slow, external) provider call — a narrow
// TOCTOU race window is accepted as a known Phase 1 MVP limitation for a
// single-task-type stub.
pub async fn evaluate(pool: &PgPool, config: &Config, ai: &dyn AIProvider, user_id: Uuid, task: &str, input: &serde_json::Value) -> Result<EvaluateResponse, AppError> {
    if task != "grammar_evaluation" {
        return Err(AppError::UnprocessableEntity("unsupported_ai_task", format!(r#"task "{task}" is not supported yet"#)));
    }
    let text = input.as_object().and_then(|o| o.get("text")).and_then(|v| v.as_str());
    let Some(text) = text else { return Err(AppError::UnprocessableEntity("invalid_ai_input", "input.text is required".to_string())) };

    // P11-004 (§6.10, "free tier benar-benar usable") — an unused ad
    // view unlocks this task WITHOUT touching the diamond balance at
    // all; only falls through to the normal credit check when there's
    // no ad view available.
    let required = config.ai_grammar_evaluation_credit_cost;
    let ad_view = ad::find_oldest_unused(pool, user_id).await?;
    let pay_with_ad = ad_view.is_some();

    if !pay_with_ad {
        economy::ensure_credits_row(pool, user_id).await?;
        let balance = economy::get_balance(pool, user_id).await?;
        if balance < required {
            return Err(AppError::InsufficientCredit { required, balance });
        }
    }

    let (system_prompt, user_prompt) = grammar_evaluation_prompt(text);
    let model = config.ai_grammar_evaluation_model.clone();
    let max_tokens = resolve_max_tokens(&model, 512).await;
    let request = GenerationRequest { model: model.clone(), system_prompt, user_prompt, temperature: 0.0, max_tokens, image_url: None, json_mode: true };

    let ai_task_id = Uuid::new_v4();

    // Provider failure and schema-validation failure both surface as the
    // same contract error (ai_output_validation_failed) — no separate
    // code for a provider outage exists, and the user-facing effect (no
    // usable result, no charge) is identical either way.
    let generation = match ai.generate(request).await {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = ?e, ai_task_id = %ai_task_id, "grammar evaluation provider call failed");
            let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "grammar_evaluation", PROVIDER, &model, GRAMMAR_EVALUATION_PROMPT_ID).await;
            return Err(AppError::AiOutputValidationFailed);
        }
    };

    let text = strip_code_fence(&generation.text);
    let result: serde_json::Value = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) if v.get("errors").and_then(|e| e.as_array()).is_some() => v,
        _ => {
            tracing::warn!(ai_task_id = %ai_task_id, raw_output = %generation.text, "grammar evaluation output failed schema validation");
            let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "grammar_evaluation", PROVIDER, &model, GRAMMAR_EVALUATION_PROMPT_ID).await;
            return Err(AppError::AiOutputValidationFailed);
        }
    };

    ai_task::insert_done(pool, ai_task_id, user_id, "grammar_evaluation", PROVIDER, &model, GRAMMAR_EVALUATION_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;

    // markConsumed's own guard (consumed_at IS NULL) means a 2nd
    // concurrent request racing for the SAME ad view returns false here
    // rather than double-spending it — that request still isn't charged
    // credit either in this MVP (same narrow TOCTOU class already
    // accepted above, not newly introduced here).
    let mut credit_charged = 0;
    if pay_with_ad {
        if let Some(ad_view) = ad_view {
            ad::mark_consumed(pool, ad_view.id).await?;
        }
    } else {
        economy::charge(pool, user_id, required, &format!("ai_task:{ai_task_id}")).await?;
        credit_charged = required;
    }

    Ok(EvaluateResponse { ai_task_id, status: "done", result, credit_charged })
}
