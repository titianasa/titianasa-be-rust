use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest};
use crate::services::ai_writing_evaluation::{FeedbackDto, PositionDto};
use crate::services::drive_permissions::DriveResource;
use crate::services::storage::AssetStorage;
use crate::services::{ai_task, assessment, evaluation};

const SPEAKING_RUBRIC_ID: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00f2);
const PROVIDER: &str = "deepseek";
const SPEAKING_EVALUATION_PROMPT_ID: &str = "speaking_evaluation_v1";
const CORRECTION_THRESHOLD: f64 = 65.0;

async fn ensure_speaking_rubric(pool: &PgPool) -> Result<evaluation::Rubric, AppError> {
    let criteria = serde_json::json!([
        {"key": "grammar", "label": "Grammar"},
        {"key": "vocabulary", "label": "Vocabulary"},
        {"key": "fluency", "label": "Fluency"},
        {"key": "naturalness", "label": "Naturalness"},
        {"key": "pronunciation", "label": "Pronunciation (approximate)"},
    ]);
    evaluation::ensure_rubric(pool, SPEAKING_RUBRIC_ID, "Speaking Evaluation v1", criteria).await
}

fn speaking_evaluation_prompt(transcript: &str) -> (String, String) {
    let system = r#"You are an ALR speaking evaluator. You only have a text TRANSCRIPT of the learner's
spoken answer, produced by automatic speech recognition — you do NOT have the audio
itself. Score exactly these 5 criteria, each 0-100: grammar, vocabulary, fluency,
naturalness, pronunciation. IMPORTANT: since you cannot hear the audio, "pronunciation"
is necessarily an APPROXIMATION inferred only from transcript artifacts (garbled or
unclear words, phonetic misspellings, filler sounds like "uh"/"um" the transcription
captured) — never imply more confidence in it than that. Output ONLY JSON, no prose,
no markdown code fences, in this exact shape (a real filled-in example, not just a
placeholder): {"scores": {"grammar": 65, "vocabulary": 72, "fluency": 58,
"naturalness": 60, "pronunciation": 55}, "feedback": [{"quote": "I want eat fried
rice", "comment": "Better: \"I'd like to have fried rice.\" — use \"want to\" or
\"would like\", not \"want\" directly before another verb."}]}
IMPORTANT: every "quote" must be an EXACT, VERBATIM substring copied from the
transcript below — never paraphrase or summarize it, it will be located by exact
string match. Include at least 2 feedback items, each pointing at a different span
of the transcript."#
        .to_string();
    let user = format!("Transcript:\n{transcript}");
    (system, user)
}

fn parse_scores(value: &serde_json::Value) -> Option<(f64, f64, f64, f64, f64)> {
    let scores = value.get("scores")?;
    let get = |key: &str| -> Option<f64> {
        let n = scores.get(key)?.as_f64()?;
        if n.is_finite() && (0.0..=100.0).contains(&n) {
            Some(n)
        } else {
            None
        }
    };
    Some((get("grammar")?, get("vocabulary")?, get("fluency")?, get("naturalness")?, get("pronunciation")?))
}

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

#[derive(Debug, serde::Serialize)]
pub struct SpeakingEvaluationScores {
    pub grammar: f64,
    pub vocabulary: f64,
    pub fluency: f64,
    pub naturalness: f64,
    pub pronunciation: f64,
    pub overall: f64,
}

#[derive(Debug, serde::Serialize)]
pub struct SpeakingEvaluationDto {
    pub id: Uuid,
    pub scores: SpeakingEvaluationScores,
    pub feedback: Vec<FeedbackDto>,
    pub correction: Option<String>,
}

pub struct SpeakingEvaluationResult {
    pub transcript: Option<String>,
    pub evaluation: Option<SpeakingEvaluationDto>,
}

async fn run_speaking_evaluation(pool: &PgPool, config: &Config, ai: &dyn AIProvider, user_id: Uuid, attempt_id: Uuid, audio_bytes: &[u8], audio_content_type: &str, question_id: Option<Uuid>) -> SpeakingEvaluationResult {
    let ai_task_id = Uuid::new_v4();
    let Ok(rubric) = ensure_speaking_rubric(pool).await else {
        return SpeakingEvaluationResult { transcript: None, evaluation: None };
    };

    let transcript = match ai.transcribe(audio_bytes, audio_content_type, &config.ai_stt_model).await {
        Ok(t) => t.text,
        Err(e) => {
            tracing::warn!(error = ?e, "speaking evaluation transcription failed");
            let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "speaking_evaluation", PROVIDER, &config.ai_speaking_evaluation_model, SPEAKING_EVALUATION_PROMPT_ID).await;
            return SpeakingEvaluationResult { transcript: None, evaluation: None };
        }
    };

    let (system_prompt, user_prompt) = speaking_evaluation_prompt(&transcript);
    let model = &config.ai_speaking_evaluation_model;
    let max_tokens = resolve_max_tokens(model, 1024).await;
    let generation = match ai.generate(GenerationRequest { model: model.clone(), system_prompt, user_prompt, temperature: 0.3, max_tokens, image_url: None, json_mode: true }).await {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = ?e, "speaking evaluation generation failed");
            let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "speaking_evaluation", PROVIDER, model, SPEAKING_EVALUATION_PROMPT_ID).await;
            return SpeakingEvaluationResult { transcript: Some(transcript), evaluation: None };
        }
    };

    let text = strip_code_fence(&generation.text);
    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "speaking_evaluation", PROVIDER, model, SPEAKING_EVALUATION_PROMPT_ID).await;
            return SpeakingEvaluationResult { transcript: Some(transcript), evaluation: None };
        }
    };

    let Some((grammar, vocabulary, fluency, naturalness, pronunciation)) = parse_scores(&parsed) else {
        let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "speaking_evaluation", PROVIDER, model, SPEAKING_EVALUATION_PROMPT_ID).await;
        return SpeakingEvaluationResult { transcript: Some(transcript), evaluation: None };
    };
    let Some(feedback) = parse_feedback(&parsed, &transcript) else {
        let _ = ai_task::insert_failed(pool, ai_task_id, user_id, "speaking_evaluation", PROVIDER, model, SPEAKING_EVALUATION_PROMPT_ID).await;
        return SpeakingEvaluationResult { transcript: Some(transcript), evaluation: None };
    };

    let overall = ((grammar + vocabulary + fluency + naturalness + pronunciation) / 5.0 * 10.0).round() / 10.0;
    let needs_correction = grammar < CORRECTION_THRESHOLD || naturalness < CORRECTION_THRESHOLD;
    let correction = if needs_correction { feedback.first().map(|f| f.comment.clone()) } else { None };

    let scores_json = serde_json::json!({"grammar": grammar, "vocabulary": vocabulary, "fluency": fluency, "naturalness": naturalness, "pronunciation": pronunciation, "overall": overall});
    let evidence = serde_json::json!({"raw_output": generation.text, "transcript": transcript});
    let Ok(inserted) = evaluation::insert_evaluation(pool, attempt_id, "ai", rubric.id, scores_json, evidence, question_id).await else {
        return SpeakingEvaluationResult { transcript: Some(transcript), evaluation: None };
    };
    let feedback_items: Vec<evaluation::FeedbackItemInput> =
        feedback.iter().map(|f| evaluation::FeedbackItemInput { content: f.comment.clone(), position: f.position.as_ref().map(|p| serde_json::json!({"start": p.start, "end": p.end})) }).collect();
    if evaluation::insert_many_feedback(pool, inserted.id, &feedback_items).await.is_err() {
        return SpeakingEvaluationResult { transcript: Some(transcript), evaluation: None };
    }
    let _ = ai_task::insert_done(pool, ai_task_id, user_id, "speaking_evaluation", PROVIDER, model, SPEAKING_EVALUATION_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await;

    SpeakingEvaluationResult {
        transcript: Some(transcript),
        evaluation: Some(SpeakingEvaluationDto { id: inserted.id, scores: SpeakingEvaluationScores { grammar, vocabulary, fluency, naturalness, pronunciation, overall }, feedback, correction }),
    }
}

#[derive(Debug, serde::Serialize)]
pub struct SpeakingSubmitResponse {
    pub attempt_id: Uuid,
    pub status: String,
    pub transcript: Option<String>,
    pub evaluation: Option<SpeakingEvaluationDto>,
}

// POST /attempts/{id}/submit — speaking branch. Access to the audio
// asset is validated BEFORE anything is accepted (unlike writing's
// "always accept then maybe fail eval") — a bad asset id/ownership
// rejects the whole submit, attempt stays in_progress.
pub async fn submit_speaking_attempt(pool: &PgPool, config: &Config, ai: &dyn AIProvider, storage: &dyn AssetStorage, ctx: &AuthContext, attempt_id: Uuid, audio_asset_id: Uuid) -> Result<SpeakingSubmitResponse, AppError> {
    crate::services::permissions::require_permission(ctx, crate::services::permissions::Resource::Attempt, crate::services::permissions::Action::Submit)?;
    let attempt = assessment::load_submittable_attempt(pool, ctx, attempt_id).await?;
    if attempt.item_id.is_none() {
        return Err(AppError::Internal(anyhow::anyhow!("attempt has no item_id")));
    }

    let access = crate::services::drive_permissions::resolve_access(pool, ctx, DriveResource::Asset, audio_asset_id).await?;
    if access.is_none() {
        return Err(AppError::NotFound("asset_not_found"));
    }
    let asset = crate::services::asset::find_by_id(pool, audio_asset_id).await?.ok_or(AppError::NotFound("asset_not_found"))?;

    assessment::submit_speaking_lesson_attempt(pool, attempt_id, audio_asset_id).await?;

    let object = match storage.get(&asset.id.to_string()).await {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = ?e, "failed to read audio asset for speaking evaluation");
            let ai_task_id = Uuid::new_v4();
            let _ = ai_task::insert_failed(pool, ai_task_id, ctx.user_id, "speaking_evaluation", PROVIDER, &config.ai_speaking_evaluation_model, SPEAKING_EVALUATION_PROMPT_ID).await;
            return Ok(SpeakingSubmitResponse { attempt_id, status: "submitted".to_string(), transcript: None, evaluation: None });
        }
    };

    let result = run_speaking_evaluation(pool, config, ai, ctx.user_id, attempt_id, &object.bytes, &object.content_type, None).await;
    let Some(evaluation) = result.evaluation else {
        return Ok(SpeakingSubmitResponse { attempt_id, status: "submitted".to_string(), transcript: result.transcript, evaluation: None });
    };
    assessment::mark_attempt_evaluated(pool, attempt_id, evaluation.scores.overall).await?;
    Ok(SpeakingSubmitResponse { attempt_id, status: "evaluated".to_string(), transcript: result.transcript, evaluation: Some(evaluation) })
}
