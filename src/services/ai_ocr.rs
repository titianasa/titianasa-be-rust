use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest};
use crate::services::drive_permissions::DriveResource;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::question::{self, OcrDraftQuestion};
use crate::services::storage::AssetStorage;
use crate::services::{ai_task, drive_permissions, question_schema};

// P2-015: OCR-to-Question Pipeline v1. An admin scans/photographs a page
// of exam questions, uploads it as a Drive asset, and this extracts it
// into draft questions via a vision-capable AI call. v1 is images only.
//
// This pipeline never auto-publishes and never silently drops an
// unclassifiable item: every extracted question — confidently
// classified or not — lands as status='draft' with an ocr_verification
// qa_report entry telling a reviewer to check it against the source
// image before it goes anywhere near submit-review/publish. An OCR
// misread on a real exam can be fatal, so this is deliberately its own
// module, not a 3rd function bolted onto ai_content.rs.

const PROVIDER: &str = "deepseek";
const OCR_PROMPT_ID: &str = "ocr_to_question_v1";

// The only 2 question_schema types this pipeline knows how to validate
// — anything else (including the model's own "unknown") is captured
// as-is rather than rejected.
const KNOWN_TYPES: [&str; 2] = ["mcq", "fill_blank"];

pub struct OcrToQuestionBlueprint {
    pub bank_id: Uuid,
    pub asset_id: Uuid,
    pub concept_ids: Vec<Uuid>,
}

fn ocr_prompt() -> (String, String) {
    let system = r#"You are an ALR OCR-to-question extractor. The attached image is a photographed or scanned page of exam/exercise questions. Output ONLY a JSON array, no prose, no markdown code fences. Extract every question visible on the page as one array item each.

For a multiple-choice question, use this exact shape (a real filled-in example, not just a placeholder): {"type": "mcq", "raw_text": "1. I ___ a student. a) am b) is c) are", "data": {"prompt": "I ___ a student.", "options": ["am", "is", "are"]}, "correct_answer": {"index": 0}, "explanation": {"text": "Use \"am\" with \"I\"."}}

For a fill-in-the-blank question: {"type": "fill_blank", "raw_text": "2. The sky is ___.", "data": {"prompt": "The sky is ___."}, "correct_answer": {"text": "blue"}, "explanation": {"text": "Sky color."}}

If a question does not clearly fit "mcq" or "fill_blank", or you cannot read it with confidence, set "type":"unknown" — still fill in "raw_text" with your best reading of it, and leave "data"/"correct_answer" as empty objects if you cannot determine them. Do NOT force an uncertain question into a category, and do NOT skip a question just because it is unclear.

"raw_text" is REQUIRED on every item: the exact text you read for that question, verbatim, so a human reviewer can check your extraction against the source image.

"correct_answer" and "explanation" are top-level fields, siblings of "data", never nested inside "data"."#.to_string();
    let user = "Extract every question from the attached image into the JSON array described above.".to_string();
    (system, user)
}

#[derive(Debug, serde::Deserialize, Default)]
struct OcrItem {
    r#type: Option<String>,
    raw_text: Option<String>,
    #[serde(default)]
    data: serde_json::Value,
    #[serde(default)]
    correct_answer: serde_json::Value,
    #[serde(default)]
    explanation: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
pub struct OcrToQuestionResponse {
    pub ai_task_id: Uuid,
    pub status: &'static str,
    pub question_ids: Vec<Uuid>,
}

async fn record_ocr_failed(pool: &PgPool, ai_task_id: Uuid, user_id: Uuid, model: &str) {
    if let Err(e) = ai_task::insert_failed(pool, ai_task_id, user_id, "ocr_to_question", PROVIDER, model, OCR_PROMPT_ID).await {
        tracing::error!(error = ?e, %ai_task_id, "failed to record failed ai_tasks row");
    }
}

// POST /ai/ocr-to-question. Auth: same as manual question creation
// (question_bank:create) — checked BEFORE the provider call so an
// unauthorized request never spends one.
pub async fn ocr_to_question(pool: &PgPool, config: &Config, ctx: &AuthContext, ai: &dyn AIProvider, model: &str, storage: &dyn AssetStorage, bp: OcrToQuestionBlueprint) -> Result<OcrToQuestionResponse, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::Create)?;

    // Real Drive permission check (not a bypass) — same resolve_access
    // asset reads use elsewhere, so OCR can only read an asset this
    // caller can actually see.
    let access = drive_permissions::resolve_access(pool, ctx, DriveResource::Asset, bp.asset_id).await?;
    if access.is_none() {
        return Err(AppError::NotFound("asset_not_found"));
    }
    let asset = crate::services::asset::find_by_id(pool, bp.asset_id).await?.ok_or(AppError::NotFound("asset_not_found"))?;

    // Re-signed fresh, not a possibly-stale url column — the provider
    // fetches this URL itself, so it must be publicly reachable (R2
    // presigned GET), not a proxy.
    let image_url = storage.signed_url(&asset.id.to_string(), config.asset_signed_url_ttl_seconds as u64).await?;

    let (system_prompt, user_prompt) = ocr_prompt();
    let max_tokens = resolve_max_tokens(model, 4096).await;
    let request = GenerationRequest { model: model.to_string(), system_prompt, user_prompt, temperature: 0.2, max_tokens, image_url: Some(image_url), json_mode: false };

    let ai_task_id = Uuid::new_v4();

    // One retry on the SAME provider — never falls back to a different
    // one — for either a raw provider failure or output that doesn't
    // parse as a non-empty JSON array.
    let mut outcome = None;
    let mut last_error = String::new();
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, "OCR provider call failed");
                last_error = format!("Provider gagal: {e}");
                continue;
            }
        };
        match serde_json::from_str::<Vec<OcrItem>>(&strip_code_fence(&generation.text)) {
            Ok(items) if !items.is_empty() => {
                outcome = Some((items, generation));
                break;
            }
            Ok(_) => {
                tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "AI-generated OCR output was an empty array");
                last_error = "Model tidak menemukan soal apa pun di gambar ini.".to_string();
            }
            Err(e) => {
                tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "AI-generated OCR output failed to parse as a JSON array");
                last_error = format!("Model tidak mengembalikan JSON array yang valid: {e}");
            }
        }
    }
    let Some((items, generation)) = outcome else {
        record_ocr_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::AiOutputValidationFailed(Some(last_error)));
    };

    // Every item lands as a draft question, confidently classified or
    // not — an OCR pipeline that silently drops a page's question
    // because it couldn't classify it would defeat the point.
    let mut question_ids = Vec::with_capacity(items.len());
    for item in items {
        let raw_text = item.raw_text.unwrap_or_default();
        let r#type = item.r#type.filter(|t| !t.is_empty()).unwrap_or_else(|| "unknown".to_string());
        let data = if item.data.is_null() { serde_json::json!({}) } else { item.data };
        let correct_answer = if item.correct_answer.is_null() { serde_json::json!({}) } else { item.correct_answer };

        let schema_ok = KNOWN_TYPES.contains(&r#type.as_str()) && question_schema::validate(&r#type, &data, &correct_answer).is_ok();

        // Every OCR-derived question carries this flag, not just the
        // uncertain ones — even a confidently-classified item still
        // needs a human to check it against the source image.
        let mut issues = vec![serde_json::json!({
            "category": "ocr_verification",
            "message": "This question was extracted by OCR — verify it against the source image before publishing.",
        })];
        if !schema_ok {
            issues.push(serde_json::json!({
                "category": "ocr_uncertain_type",
                "message": format!(r#"OCR could not confidently classify this question as a known type (model returned type="{type}") — review and reclassify manually."#),
            }));
        }
        let qa_report = serde_json::json!({"passed": false, "issues": issues, "ocr_raw_text": raw_text});

        let question_id = question::insert_ocr_draft(
            pool,
            OcrDraftQuestion { bank_id: bp.bank_id, r#type, data, correct_answer, explanation: item.explanation, qa_report, concept_ids: bp.concept_ids.clone() },
        )
        .await?;
        question_ids.push(question_id);
    }

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "ocr_to_question", PROVIDER, model, OCR_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;

    Ok(OcrToQuestionResponse { ai_task_id, status: "done", question_ids })
}
