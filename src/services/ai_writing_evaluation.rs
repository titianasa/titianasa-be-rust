use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest};
use crate::services::{ai_task, assessment, evaluation};

const WRITING_RUBRIC_ID: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00f1);
const PROVIDER: &str = "deepseek";
const WRITING_EVALUATION_PROMPT_ID: &str = "writing_evaluation_v1";

async fn ensure_writing_rubric(pool: &PgPool) -> Result<evaluation::Rubric, AppError> {
    let criteria = serde_json::json!([
        {"key": "task_achievement", "label": "Task Achievement"},
        {"key": "coherence_cohesion", "label": "Coherence & Cohesion"},
        {"key": "lexical_resource", "label": "Lexical Resource"},
        {"key": "grammar_accuracy", "label": "Grammar Accuracy"},
    ]);
    evaluation::ensure_rubric(pool, WRITING_RUBRIC_ID, "Writing Evaluation v1", criteria).await
}

fn writing_evaluation_prompt(answer_text: &str) -> (String, String) {
    let system = r#"You are an ALR writing evaluator. Score the submitted essay against exactly these
4 criteria, each 0-100: task_achievement, coherence_cohesion, lexical_resource,
grammar_accuracy. Output ONLY JSON, no prose, no markdown code fences, in this exact
shape (a real filled-in example, not just a placeholder):
{"scores": {"task_achievement": 78, "coherence_cohesion": 82, "lexical_resource": 70,
"grammar_accuracy": 75}, "feedback": [{"quote": "he go to school every day",
"comment": "Subject-verb agreement: use \"goes\", not \"go\", with \"he\"."}]}
IMPORTANT: every "quote" must be an EXACT, VERBATIM substring copied from the essay
text below — never paraphrase or summarize it, it will be located by exact string
match. Include at least 3 feedback items, each pointing at a different, specific
span of the essay — not a generic overall comment."#
        .to_string();
    let user = format!("Essay:\n{answer_text}");
    (system, user)
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct EvaluationScores {
    pub task_achievement: f64,
    pub coherence_cohesion: f64,
    pub lexical_resource: f64,
    pub grammar_accuracy: f64,
    pub overall: f64,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct FeedbackDto {
    pub quote: String,
    pub comment: String,
    pub position: Option<PositionDto>,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct PositionDto {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct EvaluationDto {
    pub id: Uuid,
    pub scores: EvaluationScores,
    pub feedback: Vec<FeedbackDto>,
}

fn parse_scores(value: &serde_json::Value) -> Option<(f64, f64, f64, f64)> {
    let scores = value.get("scores")?;
    let get = |key: &str| -> Option<f64> {
        let n = scores.get(key)?.as_f64()?;
        if n.is_finite() && (0.0..=100.0).contains(&n) {
            Some(n)
        } else {
            None
        }
    };
    Some((get("task_achievement")?, get("coherence_cohesion")?, get("lexical_resource")?, get("grammar_accuracy")?))
}

// quote/comment pairs; bad/unfindable quotes never fail the whole
// evaluation, only degrade that one item's position — but an empty
// feedback array (0 usable items) DOES fail it.
fn parse_feedback(value: &serde_json::Value, source_text: &str) -> Option<Vec<FeedbackDto>> {
    let items = value.get("feedback")?.as_array()?;
    let mut out = Vec::new();
    for item in items {
        let comment = item.get("comment").and_then(|v| v.as_str()).unwrap_or("");
        if comment.is_empty() {
            continue;
        }
        let quote = item.get("quote").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let position = if !quote.is_empty() { source_text.find(&quote).map(|start| PositionDto { start, end: start + quote.len() }) } else { None };
        out.push(FeedbackDto { quote, comment: comment.to_string(), position });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub struct WritingEvaluationResult {
    pub evaluation: EvaluationDto,
    #[allow(dead_code)]
    pub tokens_used: Option<i32>,
}

// The reusable core — also usable for a per-question evaluation inside
// a future Level Assessment scoring pass (question_id set), though that
// caller doesn't exist yet (deferred, see services::assessment's doc
// comment on submit_attempt's level_assessment branch).
pub async fn run_writing_evaluation(pool: &PgPool, ai: &dyn AIProvider, model: &str, user_id: Uuid, attempt_id: Uuid, answer_text: &str, question_id: Option<Uuid>) -> Option<WritingEvaluationResult> {
    let ai_task_id = Uuid::new_v4();
    let rubric = ensure_writing_rubric(pool).await.ok()?;
    let (system_prompt, user_prompt) = writing_evaluation_prompt(answer_text);
    let max_tokens = resolve_max_tokens(pool, model, 1024).await;

    let generation = match ai.generate(GenerationRequest { model: model.to_string(), system_prompt, user_prompt, temperature: 0.3, max_tokens, image_url: None, json_mode: true, thinking_budget: None, allow_partial: false }).await {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = ?e, "writing evaluation generation failed");
            let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "writing_evaluation", PROVIDER, model, WRITING_EVALUATION_PROMPT_ID).await;
            return None;
        }
    };

    let text = strip_code_fence(&generation.text);
    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "writing_evaluation", PROVIDER, model, WRITING_EVALUATION_PROMPT_ID).await;
            return None;
        }
    };

    let Some((task_achievement, coherence_cohesion, lexical_resource, grammar_accuracy)) = parse_scores(&parsed) else {
        let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "writing_evaluation", PROVIDER, model, WRITING_EVALUATION_PROMPT_ID).await;
        return None;
    };
    let Some(feedback) = parse_feedback(&parsed, answer_text) else {
        let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "writing_evaluation", PROVIDER, model, WRITING_EVALUATION_PROMPT_ID).await;
        return None;
    };

    let overall = ((task_achievement + coherence_cohesion + lexical_resource + grammar_accuracy) / 4.0 * 10.0).round() / 10.0;
    let scores = EvaluationScores { task_achievement, coherence_cohesion, lexical_resource, grammar_accuracy, overall };
    let scores_json = serde_json::json!({"task_achievement": task_achievement, "coherence_cohesion": coherence_cohesion, "lexical_resource": lexical_resource, "grammar_accuracy": grammar_accuracy, "overall": overall});
    let evidence = serde_json::json!({"raw_output": generation.text});

    let inserted = match evaluation::insert_evaluation(pool, attempt_id, "ai", rubric.id, scores_json, evidence, question_id).await {
        Ok(i) => i,
        Err(_) => return None,
    };
    let feedback_items: Vec<evaluation::FeedbackItemInput> =
        feedback.iter().map(|f| evaluation::FeedbackItemInput { content: f.comment.clone(), position: f.position.as_ref().map(|p| serde_json::json!({"start": p.start, "end": p.end})) }).collect();
    if evaluation::insert_many_feedback(pool, inserted.id, &feedback_items).await.is_err() {
        return None;
    }
    let _ = ai_task::insert_done(pool, ai_task_id, user_id, "writing_evaluation", PROVIDER, model, WRITING_EVALUATION_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await;

    Some(WritingEvaluationResult { evaluation: EvaluationDto { id: inserted.id, scores, feedback }, tokens_used: generation.tokens_used.map(|t| t as i32) })
}

#[derive(Debug, serde::Serialize)]
pub struct WritingSubmitResponse {
    pub attempt_id: Uuid,
    pub status: String,
    pub evaluation: Option<EvaluationDto>,
}

// POST /attempts/{id}/submit — writing branch.
pub async fn submit_writing_attempt(pool: &PgPool, ai: &dyn AIProvider, model: &str, ctx: &AuthContext, attempt_id: Uuid, answer_text: &str) -> Result<WritingSubmitResponse, AppError> {
    crate::services::permissions::require_permission(ctx, crate::services::permissions::Resource::Attempt, crate::services::permissions::Action::Submit)?;
    let attempt = assessment::load_submittable_attempt(pool, ctx, attempt_id).await?;
    if attempt.item_id.is_none() {
        return Err(AppError::Internal(anyhow::anyhow!("attempt has no item_id")));
    }
    if answer_text.trim().is_empty() {
        return Err(AppError::UnprocessableEntity("empty_writing_submission", "answer_text must not be empty".to_string()));
    }

    assessment::submit_lesson_attempt(pool, attempt_id, answer_text).await?;

    let result = run_writing_evaluation(pool, ai, model, ctx.user_id, attempt_id, answer_text, None).await;
    let Some(result) = result else {
        return Ok(WritingSubmitResponse { attempt_id, status: "submitted".to_string(), evaluation: None });
    };
    assessment::mark_attempt_evaluated(pool, attempt_id, result.evaluation.scores.overall).await?;
    Ok(WritingSubmitResponse { attempt_id, status: "evaluated".to_string(), evaluation: Some(result.evaluation) })
}
