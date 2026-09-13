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

/// A full article for one chapter runs long — several explanation blocks
/// (definition, example, common_trap, steps, comparison, ...) over one
/// topic — and the old 2048 ceiling cut that off mid-article. This asks
/// for the model's entire output budget instead; `resolve_max_tokens`
/// still clamps it to whatever the chosen model actually allows, so a
/// smaller model is not sent an impossible number.
const ARTICLE_MAX_OUTPUT_TOKENS: i64 = 65_536;

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
    // Migration 0043 — author THIS item under a different subject than
    // the module's; shapes both the prompt (which subject to write for)
    // and the created item's own subject_id.
    pub subject_id: Option<Uuid>,
}

// Subjects whose material is another LANGUAGE (as opposed to being
// taught, like every other subject, IN Indonesian). Grammar-lesson
// framing and the language-teaching ALM directives only make sense for
// these — asking the model to write a Matematika or Sejarah lesson
// under "this is a GRAMMAR lesson" instructions produced nonsense
// before this fix.
fn is_language_subject(subject_name: &str) -> bool {
    matches!(subject_name, "English" | "Bahasa Arab" | "Bahasa Asing" | "Bahasa Indonesia")
}

// Of those, the ones where "think in the target language vs Indonesian"
// is a meaningful contrast — not Bahasa Indonesia itself, which IS the
// language of instruction everywhere else.
fn is_foreign_language_subject(subject_name: &str) -> bool {
    matches!(subject_name, "English" | "Bahasa Arab" | "Bahasa Asing")
}

// Subjects where formulas/equations genuinely appear — LaTeX is worth
// mentioning in the prompt for these, and noise for e.g. Sejarah.
fn is_formula_subject(subject_name: &str) -> bool {
    matches!(
        subject_name,
        "Matematika" | "Fisika" | "Kimia" | "Biologi" | "IPA" | "IPAS" | "Informatika"
            | "Akuntansi" | "Ekonomi" | "Penalaran & Logika"
    )
}

fn lesson_generation_prompt(bp: &LessonGenerationBlueprint, subject_name: &str) -> (String, String) {
    let language = is_language_subject(subject_name);
    let foreign_language = is_foreign_language_subject(subject_name);
    let formula = is_formula_subject(subject_name);

    // A flashcard/common_trap example pair suited to what's actually
    // being taught — a foreign-language translation pair makes no
    // sense for Matematika, and a math-flavoured example makes no
    // sense for English, so this varies by subject family.
    let (flashcard_front, flashcard_back, trap_term, trap_explanation): (&str, &str, &str, &str) = if foreign_language {
        ("go", "went", "actual", "means real/genuine in English, not \"aktual\" (current/topical)")
    } else if language {
        (
            "kalimat efektif",
            "kalimat yang lugas, jelas, dan tidak bertele-tele",
            "di mana",
            "kata depan (dua kata) — berbeda dari \"dimana\" (kata tanya, satu kata) yang sering tertukar",
        )
    } else {
        ("istilah kunci mata pelajaran ini", "definisi singkatnya", "miskonsepsi umum siswa", "penjelasan singkat kenapa itu keliru")
    };

    let mut system = format!(
        "You are an ALR curriculum content generator, writing a lesson for the subject \"{subject_name}\" \
in the Indonesian K-12/tertiary curriculum. Output ONLY ALR Learning Markdown (ALM) content \
— no prose outside the lesson, no markdown code fences around the whole output. Use \"# \"/\"## \" \
headings and \"> \" blockquotes for single-line examples for most of the lesson. Write the \
lesson itself in Indonesian, the language every other subject on this platform is taught in \
(quote target-language examples verbatim where the topic calls for them).\n\n\
You may ALSO use \":::type\\nkey: value\\n:::\" directive blocks, but ONLY the text-only types \
below, each with EXACTLY these keys, no others — every directive line must be \"key: value\", \
never a bare line with no key:\n\
:::flashcard\n\
front: {flashcard_front}\n\
back: {flashcard_back}\n\
:::\n\
:::common_trap\n\
term: {trap_term}\n\
explanation: {trap_explanation}\n\
:::\n"
    );

    if language {
        system.push_str(
            ":::indonesian_learner_alert\n\
text: A specific mistake Indonesian learners of this subject commonly make, and why.\n\
:::\n",
        );
    }
    if foreign_language {
        system.push_str(&format!(
            ":::think_in_english\n\
indonesian_pattern: contoh pola berpikir dalam Bahasa Indonesia\n\
english_pattern: the equivalent pattern in {subject_name}\n\
:::\n"
        ));
    }

    if formula {
        system.push_str(&format!(
            "\n\nThis subject uses mathematical notation. Write any formula, equation, or \
symbolic expression as LaTeX inside a plain paragraph or heading — inline as \
$...$ (e.g. \"turunan dari $x^2$ adalah $2x$\") or, for a standalone equation on its \
own line, display math as $$...$$ (e.g. $$\\int_0^1 x^2\\,dx = \\frac{{1}}{{3}}$$). This is \
plain text, not a directive — do not wrap it in a :::type block. Prefer this over ASCII \
approximations (\"x^2\" or \"sqrt(x)\") every time an actual formula appears, since {subject_name} \
content is unreadable without properly typeset notation."
        ));
    }

    if foreign_language || subject_name == "Bahasa Indonesia" {
        system.push_str(&format!(
            "\n\nWrite any {subject_name} example, phrase, or vocabulary item in its own actual \
script — Arabic script for Bahasa Arab, Hanzi/Pinyin for Mandarin, Hangul for Korean, Kana/\
Kanji for Japanese, and so on, matching whichever language this topic is actually about. \
Never transliterate into Latin letters as a substitute for the real script; give a Latin \
transliteration or Indonesian gloss ALONGSIDE the original script, not instead of it."
        ));
    }

    system.push_str(
        "\nDo NOT use audio/video/image/question_embed directives — you have no real \
asset or question ids to reference, so any use of them will be invalid. Every directive \
above is optional; when in doubt, prefer plain headings/paragraphs/blockquotes instead \
of a directive.",
    );

    if language {
        if let Some(_grammar_target) = &bp.grammar_target {
            system.push_str(
                "\n\nThis is a GRAMMAR lesson. It MUST contain a heading for every one of the \
following 11 sections, in order, each heading's text starting with the exact \
numeric prefix shown (e.g. a heading literally starting with \"01 — \"):\n",
            );
            for (prefix, title) in curriculum_constitution::SECTIONS {
                system.push_str(&format!("{prefix} — {title}\n"));
            }
        }
    }

    let mut user = format!("Subject: {subject_name}\nTopic: {}\nLesson type: {}\n", bp.topic, bp.content_type);
    if language {
        if let Some(grammar_target) = &bp.grammar_target {
            user.push_str(&format!("Grammar target: {grammar_target}\n"));
        }
    }
    if !bp.vocab_target.is_empty() {
        let label = if language { "Vocabulary target" } else { "Istilah kunci (key terms) to cover" };
        user.push_str(&format!("{label}: {}\n", bp.vocab_target.join(", ")));
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

    // The prompt is shaped by which subject this module actually belongs
    // to (Matematika vs English vs Sejarah, ...) — falls back to a
    // neutral label rather than failing outright, since a folder module
    // has no subject_id of its own.
    // The item's own subject_id wins when set (this generated item is
    // being authored for a DIFFERENT subject than its module's) — same
    // "override beats inherited" precedence module_item::update_subject
    // and item-editor-pane.tsx's effective-subject resolution both use.
    let effective_subject_id = match bp.subject_id {
        Some(id) => Some(id),
        None => sqlx::query_scalar!(r#"select subject_id from modules where id = $1"#, bp.module_id)
            .fetch_optional(pool)
            .await?
            .flatten(),
    };
    let subject_name = match effective_subject_id {
        Some(id) => sqlx::query_scalar!(r#"select name from subjects where id = $1"#, id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_else(|| "materi umum".to_string()),
        None => "materi umum".to_string(),
    };

    let (system_prompt, user_prompt) = lesson_generation_prompt(&bp, &subject_name);
    let max_tokens = resolve_max_tokens(pool, model, ARTICLE_MAX_OUTPUT_TOKENS).await;
    let request = GenerationRequest { model: model.to_string(), system_prompt, user_prompt, temperature: 0.4, max_tokens, image_url: None, json_mode: false };

    let ai_task_id = Uuid::new_v4();

    // One retry on the SAME provider across the whole generate-then-
    // validate pipeline — a parse failure or a Grammar Constitution miss
    // is just as often one-off model flakiness as a raw provider error,
    // and none of this pipeline writes to the DB until it all passes.
    // Never falls back to a different provider or model.
    let mut outcome = None;
    let mut last_error = String::new();
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, "lesson generation provider call failed");
                last_error = format!("Provider gagal: {e}");
                continue;
            }
        };
        let alm = strip_code_fence(&generation.text);
        let parsed = match alm_parser::parse(&alm) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(%ai_task_id, attempt, raw_output = %alm, "AI-generated ALM failed to parse");
                last_error = format!("{e}");
                continue;
            }
        };
        let new_blocks: Vec<NewContentBlock> =
            parsed.iter().enumerate().map(|(i, b)| NewContentBlock { r#type: b.r#type.clone(), order_index: i as i32, data: b.data.clone(), raw_source: Some(b.raw_source.clone()) }).collect();
        if let Err(e) = content_block::validate_blocks(pool, &new_blocks).await {
            tracing::warn!(error = ?e, %ai_task_id, attempt, "AI-generated lesson failed block schema validation");
            last_error = format!("{e}");
            continue;
        }
        if let Some(typed_err) = bp.grammar_target.as_ref().and_then(|_| {
            let typed: Vec<curriculum_constitution::TypedBlock> = parsed.iter().map(|b| curriculum_constitution::TypedBlock { r#type: b.r#type.clone(), data: b.data.clone() }).collect();
            curriculum_constitution::validate_grammar_lesson(&typed).err()
        }) {
            tracing::warn!(error = ?typed_err, %ai_task_id, attempt, "AI-generated lesson failed Grammar Constitution");
            last_error = format!("{typed_err}");
            continue;
        }
        outcome = Some((alm, generation));
        break;
    }
    let Some((alm, generation)) = outcome else {
        record_lesson_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::AiOutputValidationFailed(Some(last_error)));
    };

    // Writing into a bab's existing "Pembahasan" article — the usual
    // path once a topic's structure is authored ahead of its content.
    // update_content runs the same alm_parser/block_schema pipeline and
    // enforces the same ADR-0008 published-immutable rule.
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
        // This path only ever generates "article" content (ALM prose,
        // parsed straight into content_blocks) — quiz generation is a
        // separate flow producing a quiz_config JSON, not ALM.
        quiz_config: None,
        // The raw override, not effective_subject_id — a plain None
        // here correctly means "inherit the module's", same as manual
        // item creation; storing a redundant copy of the module's own
        // subject_id on every item would make module_item::update_subject's
        // "None = inherit" contract ambiguous with "None = never checked".
        subject_id: bp.subject_id,
    };
    let item = module_item::create_with_provenance(pool, ctx, bp.module_id, new_item, "ai").await?;

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "lesson_generation", PROVIDER, model, LESSON_GENERATION_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;

    Ok(GenerateLessonResponse { ai_task_id, status: "done", item_id: item.id })
}

// --- Question generation ---

pub struct QuestionGenerationBlueprint {
    pub bank_id: Uuid,
    // Must be a type registered in question_schema's registry —
    // multiple_choice/gap_fill/matching at time of writing.
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
    let max_tokens = resolve_max_tokens(pool, model, (400 * bp.count).max(512)).await;
    let request = GenerationRequest { model: model.to_string(), system_prompt, user_prompt, temperature: 0.4, max_tokens, image_url: None, json_mode: false };

    let ai_task_id = Uuid::new_v4();

    // One retry on the SAME provider across generate + parse + schema
    // validation — never falls back to a different provider or model.
    // A count mismatch is NOT retried: that's the model ignoring an
    // explicit instruction, not flakiness a second identical prompt
    // reliably fixes, and it already carries its own descriptive error.
    let mut outcome = None;
    let mut last_error = String::new();
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, "question generation provider call failed");
                last_error = format!("Provider gagal: {e}");
                continue;
            }
        };
        let items: Vec<GeneratedQuestionItem> = match serde_json::from_str(&strip_code_fence(&generation.text)) {
            Ok(items) => items,
            Err(e) => {
                tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "AI-generated question output failed to parse as a JSON array");
                last_error = format!("Model tidak mengembalikan JSON array yang valid: {e}");
                continue;
            }
        };
        if items.len() as i64 != bp.count {
            tracing::warn!(%ai_task_id, attempt, expected = bp.count, got = items.len(), "AI-generated question count mismatch");
            record_question_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
            return Err(AppError::UnprocessableEntity("invalid_ai_output_count", format!("expected {} question(s), got {}", bp.count, items.len())));
        }
        // Validate every item against the schema registry BEFORE writing
        // any of them — one bad item fails the whole batch, no partial
        // set of questions ever gets created.
        let invalid = items.iter().find_map(|item| question_schema::validate(&bp.question_type, &item.data, &item.correct_answer).err());
        if let Some(e) = invalid {
            tracing::warn!(error = ?e, %ai_task_id, attempt, "AI-generated question failed schema validation");
            last_error = format!("{e}");
            continue;
        }
        outcome = Some((items, generation));
        break;
    }
    let Some((items, generation)) = outcome else {
        record_question_generation_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::AiOutputValidationFailed(Some(last_error)));
    };

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
