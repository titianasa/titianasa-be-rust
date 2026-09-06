use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::module::CreateModuleItemRequest;
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest};
use crate::services::content_block::NewContentBlock;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::{ai_task, alm_parser, content_block, curriculum_constitution, module_item, question, question_schema};

// P2-013: AI Content Generation Pipeline v1 — Blueprint -> Generate ->
// Validate -> (QA Agent at submit-review) -> Human Review -> Publish.
// Per ADR-0005: LessonGeneration/QuestionGeneration are platform cost,
// not user cost — 0 credit charged, unlike ai_gateway.rs::evaluate this
// never touches economy.rs.
//
// Both generation paths validate the AI's raw output BEFORE writing
// anything to lessons/questions — a failed generation must never leave a
// half-created row behind (P2-013 DoD). ai_tasks still records every
// attempt, success or failure, for audit (ADR-0004).

const PROVIDER: &str = "deepseek";
const LESSON_GENERATION_PROMPT_ID: &str = "lesson_generation_v1";
const QUESTION_GENERATION_PROMPT_ID: &str = "question_generation_v1";

// --- Lesson generation ---

pub struct LessonGenerationBlueprint {
    pub module_id: Uuid,
    // Which section of the module this generated item lands under, if
    // any — None places it at the module's root.
    pub parent_id: Option<Uuid>,
    pub content_type: String,
    pub topic: String,
    // Set marks this as a grammar lesson — triggers the Grammar
    // Constitution check on the generated output, and tells the model
    // the specific pattern to teach.
    pub grammar_target: Option<String>,
    pub vocab_target: Vec<String>,
    pub concept_ids: Vec<Uuid>,
}

fn lesson_generation_prompt(bp: &LessonGenerationBlueprint) -> (String, String) {
    let mut system = String::from(
        "You are an ALR curriculum content generator. Output ONLY ALR Learning Markdown \
(ALM) content — no prose outside the lesson, no markdown code fences around the \
whole output. Use \"# \"/\"## \" headings and \"> \" blockquotes for single-line \
examples for most of the lesson.\n\n\
You may ALSO use \":::type\\nkey: value\\n:::\" directive blocks, but ONLY the \
text-only types below, each with EXACTLY these keys, no others — every directive \
line must be \"key: value\", never a bare line with no key:\n\
:::flashcard\n\
front: go\n\
back: went\n\
:::\n\
:::indonesian_learner_alert\n\
text: Indonesian doesn't mark tense with a verb change like this.\n\
:::\n\
:::common_trap\n\
term: actual\n\
explanation: means real/genuine in English, not \"aktual\" (current/topical).\n\
:::\n\
:::think_in_english\n\
indonesian_pattern: Saya sudah makan (kata kerja tidak berubah)\n\
english_pattern: I have eaten (verb changes to \"have + V3\")\n\
:::\n\n\
Do NOT use audio/video/image/question_embed directives — you have no real \
asset or question ids to reference, so any use of them will be invalid. Every \
directive above is optional; when in doubt, prefer plain headings/paragraphs/\
blockquotes instead of a directive.",
    );

    if bp.grammar_target.is_some() {
        system.push_str(
            "\n\nThis is a GRAMMAR lesson. It MUST contain a heading for every one of the \
following 11 sections, in order, each heading's text starting with the exact \
numeric prefix shown (e.g. a heading literally starting with \"01 — \"):\n",
        );
        for (prefix, title) in curriculum_constitution::SECTIONS {
            system.push_str(&format!("{prefix} — {title}\n"));
        }
    }

    let mut user = format!("Topic: {}\nLesson type: {}\n", bp.topic, bp.content_type);
    if let Some(grammar_target) = &bp.grammar_target {
        user.push_str(&format!("Grammar target: {grammar_target}\n"));
    }
    if !bp.vocab_target.is_empty() {
        user.push_str(&format!("Vocabulary target: {}\n", bp.vocab_target.join(", ")));
    }

    (system, user)
}

#[derive(Debug, serde::Serialize)]
pub struct GenerateLessonResponse {
    pub ai_task_id: Uuid,
    pub status: &'static str,
    pub item_id: Uuid,
}

async fn record_lesson_generation_failed(pool: &PgPool, ai_task_id: Uuid, user_id: Uuid, model: &str) {
    if let Err(e) = ai_task::insert_failed(pool, ai_task_id, user_id, "lesson_generation", PROVIDER, model, LESSON_GENERATION_PROMPT_ID).await {
        tracing::error!(error = ?e, %ai_task_id, "failed to record failed ai_tasks row");
    }
}

// POST /ai/generate-lesson. Auth: same as manual lesson creation
// (lesson:create) — checked BEFORE the AI call so an unauthorized
// request never spends a provider call.
pub async fn generate_lesson(pool: &PgPool, ctx: &AuthContext, ai: &dyn AIProvider, model: &str, bp: LessonGenerationBlueprint) -> Result<GenerateLessonResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;

    let (system_prompt, user_prompt) = lesson_generation_prompt(&bp);
    let max_tokens = resolve_max_tokens(model, 2048).await;
    let request = GenerationRequest { model: model.to_string(), system_prompt, user_prompt, temperature: 0.4, max_tokens, image_url: None, json_mode: false };

    let ai_task_id = Uuid::new_v4();

    let generation = match ai.generate(request).await {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = ?e, %ai_task_id, "lesson generation provider call failed");
            record_lesson_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
            return Err(AppError::AiOutputValidationFailed);
        }
    };

    let alm = strip_code_fence(&generation.text);

    // Validate parse-ability, block schema, and — if applicable — the
    // Grammar Constitution, all BEFORE any DB write, so a failed
    // generation never leaves an orphan lessons row behind.
    let parsed = match alm_parser::parse(&alm) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(%ai_task_id, raw_output = %alm, "AI-generated ALM failed to parse");
            record_lesson_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
            return Err(e);
        }
    };

    let new_blocks: Vec<NewContentBlock> =
        parsed.iter().enumerate().map(|(i, b)| NewContentBlock { r#type: b.r#type.clone(), order_index: i as i32, data: b.data.clone(), raw_source: Some(b.raw_source.clone()) }).collect();

    if let Err(e) = content_block::validate_blocks(pool, &new_blocks).await {
        tracing::warn!(error = ?e, %ai_task_id, "AI-generated lesson failed block schema validation");
        record_lesson_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(e);
    }

    if bp.grammar_target.is_some() {
        let typed: Vec<curriculum_constitution::TypedBlock> = parsed.iter().map(|b| curriculum_constitution::TypedBlock { r#type: b.r#type.clone(), data: b.data.clone() }).collect();
        if let Err(e) = curriculum_constitution::validate_grammar_lesson(&typed) {
            tracing::warn!(error = ?e, %ai_task_id, "AI-generated lesson failed Grammar Constitution");
            record_lesson_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
            return Err(e);
        }
    }

    // Everything above is a pure re-derivation of what module_item::create
    // will compute again internally (alm_parser/block_schema are pure
    // functions) — this call is expected to succeed given it just
    // passed the same checks.
    let new_item = CreateModuleItemRequest {
        parent_id: bp.parent_id,
        node_type: "item".to_string(),
        title: bp.topic,
        content_type: Some(bp.content_type),
        content: Some(alm),
        format: Some("markdown".to_string()),
        concept_ids: Some(bp.concept_ids),
    };
    let item = module_item::create_with_provenance(pool, ctx, bp.module_id, new_item, "ai").await?;

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "lesson_generation", PROVIDER, model, LESSON_GENERATION_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;

    Ok(GenerateLessonResponse { ai_task_id, status: "done", item_id: item.id })
}

// --- Question generation ---

pub struct QuestionGenerationBlueprint {
    pub bank_id: Uuid,
    // Must be a type registered in question_schema's registry — mcq/
    // fill_blank/matching at time of writing.
    pub question_type: String,
    pub topic: String,
    pub count: i64,
    pub difficulty: f64,
    pub concept_ids: Vec<Uuid>,
}

// A fully filled-in example (not just an abstract shape placeholder) —
// a real smoke test showed the model nesting correct_answer *inside*
// data despite the shape being spelled out; a concrete example plus an
// explicit anti-nesting warning is the fix that already worked for
// lesson generation's ALM syntax.
fn question_type_shape_hint(question_type: &str) -> &'static str {
    match question_type {
        "mcq" => r#"{"data": {"prompt": "I ___ a student.", "options": ["am", "is", "are"]}, "correct_answer": {"index": 0}, "explanation": {"text": "Use \"am\" with \"I\"."}}"#,
        "fill_blank" => r#"{"data": {"prompt": "The sky is ___."}, "correct_answer": {"text": "blue"}, "explanation": {"text": "Sky color."}}"#,
        "matching" => r#"{"data": {"pairs": [["apple", "fruit"], ["dog", "animal"]]}, "correct_answer": {"pairs": [["apple", "fruit"], ["dog", "animal"]]}, "explanation": {"text": "Category matching."}}"#,
        _ => r#"{"data": {}, "correct_answer": {}, "explanation": {"text": "string"}}"#,
    }
}

fn question_generation_prompt(bp: &QuestionGenerationBlueprint) -> (String, String) {
    let system = format!(
        "You are an ALR question bank generator. Output ONLY a JSON array of exactly \
{} items, no prose, no markdown code fences. Every item must have this exact \
shape for type=\"{}\" (this is a real filled-in example, not just a \
placeholder): {}\n\
IMPORTANT: \"correct_answer\" and \"explanation\" are top-level fields, siblings \
of \"data\" — never nest them inside \"data\".",
        bp.count,
        bp.question_type,
        question_type_shape_hint(&bp.question_type),
    );
    let user = format!("Topic: {}\nQuestion type: {}\nDifficulty (0.0-1.0): {}\nGenerate exactly {} question(s).", bp.topic, bp.question_type, bp.difficulty, bp.count);
    (system, user)
}

#[derive(Debug, serde::Deserialize)]
struct GeneratedQuestionItem {
    data: serde_json::Value,
    correct_answer: serde_json::Value,
    #[serde(default)]
    explanation: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
pub struct GenerateQuestionsResponse {
    pub ai_task_id: Uuid,
    pub status: &'static str,
    pub question_ids: Vec<Uuid>,
}

async fn record_question_generation_failed(pool: &PgPool, ai_task_id: Uuid, user_id: Uuid, model: &str) {
    if let Err(e) = ai_task::insert_failed(pool, ai_task_id, user_id, "question_generation", PROVIDER, model, QUESTION_GENERATION_PROMPT_ID).await {
        tracing::error!(error = ?e, %ai_task_id, "failed to record failed ai_tasks row");
    }
}

// POST /ai/generate-questions. Unlike lesson generation, the model
// outputs Semantic JSON directly — a deliberately separate prompt/
// parsing path from generate_lesson, never merged into one template.
// `json_mode` is deliberately NEVER set here: the expected output is a
// top-level JSON ARRAY, and jsonMode silently collapses array output
// into an object on some providers (R8-era finding, re-confirmed here).
// Auth: same as manual question creation (question_bank:create).
pub async fn generate_questions(pool: &PgPool, ctx: &AuthContext, ai: &dyn AIProvider, model: &str, bp: QuestionGenerationBlueprint) -> Result<GenerateQuestionsResponse, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::Create)?;

    if bp.count < 1 {
        return Err(AppError::UnprocessableEntity("invalid_ai_blueprint", "count must be at least 1".to_string()));
    }

    let (system_prompt, user_prompt) = question_generation_prompt(&bp);
    let max_tokens = resolve_max_tokens(model, (400 * bp.count).max(512)).await;
    let request = GenerationRequest { model: model.to_string(), system_prompt, user_prompt, temperature: 0.4, max_tokens, image_url: None, json_mode: false };

    let ai_task_id = Uuid::new_v4();

    let generation = match ai.generate(request).await {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = ?e, %ai_task_id, "question generation provider call failed");
            record_question_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
            return Err(AppError::AiOutputValidationFailed);
        }
    };

    let items: Vec<GeneratedQuestionItem> = match serde_json::from_str(&strip_code_fence(&generation.text)) {
        Ok(items) => items,
        Err(_) => {
            tracing::warn!(%ai_task_id, raw_output = %generation.text, "AI-generated question output failed to parse as a JSON array");
            record_question_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
            return Err(AppError::AiOutputValidationFailed);
        }
    };

    if items.len() as i64 != bp.count {
        tracing::warn!(%ai_task_id, expected = bp.count, got = items.len(), "AI-generated question count mismatch");
        record_question_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::UnprocessableEntity("invalid_ai_output_count", format!("expected {} question(s), got {}", bp.count, items.len())));
    }

    // Validate every item against the schema registry BEFORE writing any
    // of them — one bad item fails the whole batch, no partial set of
    // questions ever gets created.
    for item in &items {
        if let Err(e) = question_schema::validate(&bp.question_type, &item.data, &item.correct_answer) {
            tracing::warn!(error = ?e, %ai_task_id, "AI-generated question failed schema validation");
            record_question_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
            return Err(e);
        }
    }

    let mut question_ids = Vec::with_capacity(items.len());
    for item in items {
        let new_question = question::NewQuestion {
            bank_id: bp.bank_id,
            r#type: bp.question_type.clone(),
            difficulty: bp.difficulty,
            data: item.data,
            correct_answer: item.correct_answer,
            explanation: item.explanation,
            concept_ids: bp.concept_ids.clone(),
            // AI generation doesn't determine a skill_category — out of
            // scope for that pipeline, tag manually afterward if needed.
            skill_category: None,
            generated_by: "ai".to_string(),
        };
        let result = question::create_question(pool, ctx, new_question).await?;
        question_ids.push(result.id);
    }

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "question_generation", PROVIDER, model, QUESTION_GENERATION_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;

    Ok(GenerateQuestionsResponse { ai_task_id, status: "done", question_ids })
}
