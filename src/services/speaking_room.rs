use sqlx::PgPool;

use crate::errors::AppError;
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest, SpeechResult};

// P29-001 — Speaking Room: free-form multi-language AI conversation
// practice. Deliberately stateless — no DB persistence, no credit
// charge, no ai_tasks audit row. The full conversation history +
// language/scenario/tutor selection lives client-side.

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ScenarioInput {
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TutorPersonaInput {
    pub name: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct LanguageInput {
    pub code: String,
    pub name: String,
    #[serde(rename = "localName")]
    pub local_name: Option<String>,
    #[serde(rename = "indonesianName")]
    pub indonesian_name: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "writingSystem")]
    pub writing_system: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct HistoryEntry {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct TurnRequest {
    pub scenario: Option<ScenarioInput>,
    pub level: Option<String>,
    pub user_message: String,
    pub history: Option<Vec<HistoryEntry>>,
    pub tutor_persona: Option<TutorPersonaInput>,
    pub mode: Option<String>,
    pub language: Option<LanguageInput>,
}

fn ai_output_validation_failed() -> AppError {
    AppError::UnprocessableEntity("ai_output_validation_failed", "AI output failed validation".to_string())
}

// Ported near-verbatim from speaking-main/server.ts's systemPrompt — the
// per-script transliteration rules are the actual reason this feature is
// worth porting language-by-language rather than rebuilding from scratch.
fn turn_system_prompt(req: &TurnRequest) -> String {
    let is_study_mode = req.mode.as_deref() == Some("study");
    let tutor_name = req.tutor_persona.as_ref().and_then(|t| t.name.as_deref()).unwrap_or("AI Tutor");
    let lang_code = req.language.as_ref().map(|l| l.code.as_str()).unwrap_or("en");
    let lang_name = req.language.as_ref().map(|l| l.name.as_str()).unwrap_or("English");
    let lang_local_name = req.language.as_ref().and_then(|l| l.local_name.as_deref()).unwrap_or(lang_name);
    let default_indonesian = format!("Bahasa {lang_name}");
    let lang_indonesian = req.language.as_ref().and_then(|l| l.indonesian_name.as_deref()).unwrap_or(&default_indonesian);
    let level = req.level.as_deref().unwrap_or("Intermediate B1");
    let scenario_title = req.scenario.as_ref().and_then(|s| s.title.as_deref()).unwrap_or("Daily Conversation");
    let scenario_description = req.scenario.as_ref().and_then(|s| s.description.as_deref()).unwrap_or("Free talking");
    let mode_label = if is_study_mode { "MODE BELAJAR (STUDY MODE)" } else { "MODE PERCAKAPAN (CONVERSATION MODE)" };

    format!(
        r#"You are {tutor_name}, an expert native conversation partner and dedicated bilingual language coach in {lang_name} ({lang_local_name} / {lang_indonesian}).
User Target Level: {level}
Topic / Scenario: {scenario_title} - {scenario_description}
Current Mode: {mode_label}
Target Language: {lang_name} ({lang_code})

LANGUAGE SCRIPT & TRANSLITERATION RULES:
- If Target Language is Mandarin Chinese (zh):
  * All Chinese text MUST be written in simplified Hanzi characters.
  * You MUST provide Pinyin with tone marks (e.g. Nǐ hǎo, xièxie) in the transliteration fields (correctedTransliteration, naturalTransliteration, etc.).
  * For pronunciation and grammar: focus on tone accuracy (1st, 2nd, 3rd, 4th tones), tone sandhi (e.g. 3rd+3rd tone, bù, yī), measure words (量词), and grammar particles (了, 吗, 呢, 吧, 的/得/地).
- If Target Language is Korean (ko):
  * All Korean text MUST be written in Hangul.
  * Provide Revised Romanization in transliteration fields.
  * Focus on speech politeness level (존댓말 해요체/하십시오체 vs 반말), subject/object particles (은/는, 이/가, 을/를), and batchim (받침) pronunciation assimilation rules.
- If Target Language is Arabic (ar):
  * All Arabic text MUST be written in proper Arabic script with Harakat/Tashkeel (تَشْكِيل).
  * Provide Latin transliteration in transliteration fields.
  * Focus on makharijul huruf (ع, غ, ق, ح, خ, ص, ض, ط, ظ), letter length (Mad), gender agreement (mudzakkar/mu'annats), and modern conversational Arabic.
- If Target Language is Japanese (ja):
  * Write with Kanji + Kana.
  * Provide Romaji in transliteration fields.
  * Focus on polite forms (desu/masu), particles (wa, ga, o, ni, de), and pitch accent.
- If Target Language is English (en), French (fr), Spanish (es), or German (de):
  * Write in standard natural native spelling with proper diacritics/accents.
  * Provide IPA/phonetics and language-specific rules (liaison/silent endings for French, ser/estar & rolled R for Spanish, der/die/das cases & word order for German, tense/prepositions for English).

CORE OBJECTIVES:
1. SPOKEN INDONESIAN TUTOR COACHING (spokenFeedbackIndonesian):
   - Provide high-value, warm, and highly educational spoken audio coaching in BAHASA INDONESIA (2 to 4 sentences).
   - This audio will be played aloud to the user in Study Mode before the {lang_name} conversation turn begins.
   - Flow:
     a. Acknowledge and validate the user's attempt in {lang_indonesian}.
     b. Give an exact, spoken correction: "Dalam {lang_indonesian}, versi yang lebih tepat dan alami adalah: '[corrected phrase in target language]' (artinya: ...), karena [penjelasan tata bahasa/partikel/nada singkat]."
     c. Give a quick spoken tip on pronunciation/tone/makhraj: "Perhatikan juga pelafalan '[word]', cara mengucapkannya ...".
     d. End with an encouraging invitation to hear the partner's reply: "Yuk dengarkan respon percakapannya berikut ini dan coba lanjutkan ya!"

2. {upper_lang_name} CONVERSATION PARTNER RESPONSE (reply):
   - Act as a genuine, fluent native conversation partner speaking in natural {lang_name} (2 to 4 sentences).
   - Actively listen and respond directly to the student's exact ideas in "{user_message}".
   - Keep the dialogue engaging and dynamic by sharing a relatable thought and asking a thoughtful follow-up question in {lang_name}.

3. IN-DEPTH ANALYTICAL FEEDBACK (feedback):
   - Inspect: "{user_message}".
   - Grammar: Identify errors or awkward phrasing. Provide ruleName, originalFragment, correctedFragment, full corrected sentence, correctedTransliteration (if non-Latin), clear explanation in Indonesian, and a handy formula/rule tip.
   - correctedVersion: correctedSentence, correctedTransliteration, naturalAlternative, naturalTransliteration, whyNaturalIndonesian.
   - vocabulary: 1-2 words/idioms with originalWord, word, transliteration, Indonesian meaning, whyBetter, and example sentence with transliteration.
   - pronunciation: 1-2 words with word, transliteration, phonetic, syllableStress (or tone/pitch guide), tongue/mouth placement tip in Indonesian, and indonesianCommonPitfall.
   - fluencyScore (1-100), cefrEstimate (A1-C1), and indonesianNote.

Respond with ONLY a single JSON object (no markdown fences), matching exactly this shape:
{{"reply": string, "feedback": {{"spokenFeedbackIndonesian": string, "grammar": {{"hasError": boolean, "corrected": string, "correctedTransliteration"?: string, "explanation": string, "ruleName"?: string, "originalFragment"?: string, "correctedFragment"?: string, "ruleFormula"?: string}}, "betterAlternative": string, "betterAlternativeTransliteration"?: string, "correctedVersion": {{"correctedSentence": string, "correctedTransliteration"?: string, "naturalAlternative": string, "naturalTransliteration"?: string, "whyNaturalIndonesian": string}}, "vocabulary": [{{"word": string, "transliteration"?: string, "meaning": string, "example": string, "exampleTransliteration"?: string, "originalWord"?: string, "whyBetter"?: string}}], "pronunciation": [{{"word": string, "transliteration"?: string, "phonetic": string, "tip": string, "syllableStress"?: string, "indonesianCommonPitfall"?: string}}], "fluencyScore": number, "cefrEstimate": string, "indonesianNote": string}}}}"#,
        upper_lang_name = lang_name.to_uppercase(),
        user_message = req.user_message,
    )
}

fn turn_user_prompt(req: &TurnRequest) -> String {
    let lang_name = req.language.as_ref().map(|l| l.name.as_str()).unwrap_or("English");
    let history = req.history.as_deref().unwrap_or(&[]);
    let start = history.len().saturating_sub(10);
    let conversation_context = history[start..]
        .iter()
        .map(|m| format!("{}: \"{}\"", if m.role == "user" { "Student" } else { "Tutor" }, m.content))
        .collect::<Vec<_>>()
        .join("\n");
    let conversation_context = if conversation_context.is_empty() { "Start of conversation.".to_string() } else { conversation_context };

    format!(
        "Previous conversation flow:\n{conversation_context}\n\nStudent's latest spoken sentence in {lang_name}:\n\"{}\"\n\nGenerate in-depth Indonesian spoken tutor feedback, the conversational {lang_name} response, and thorough analytical corrections.",
        req.user_message,
    )
}

// Returns the AI's raw parsed JSON object untouched (camelCase field
// names, arbitrary optional nested fields) — the wire response for this
// endpoint is a deliberate exception to the codebase's snake_case
// convention, since the handler forwards the model's own JSON output
// verbatim rather than re-keying it into a typed DTO.
pub async fn generate_turn(pool: &PgPool, ai: &dyn AIProvider, model: &str, req: TurnRequest) -> Result<serde_json::Value, AppError> {
    if req.user_message.trim().is_empty() {
        return Err(AppError::UnprocessableEntity("user_message_required", "userMessage must not be empty".to_string()));
    }

    let max_tokens = resolve_max_tokens(pool, model, 4096).await;
    let generation = ai
        .generate(GenerationRequest {
            model: model.to_string(),
            system_prompt: turn_system_prompt(&req),
            user_prompt: turn_user_prompt(&req),
            temperature: 0.8,
            max_tokens,
            image_url: None,
            // P29-002 finding: response_format:{type:"json_object"}
            // keeps the model's chain-of-thought reasoning out of
            // `content`, confirmed live on a previously-failing Arabic
            // turn.
            json_mode: true,
        })
        .await
        .map_err(|e| {
            tracing::warn!(error = ?e, "speaking room turn generation failed");
            ai_output_validation_failed()
        })?;

    let text = strip_code_fence(&generation.text);
    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        tracing::warn!(error = ?e, text, "speaking room turn output failed to parse as JSON");
        ai_output_validation_failed()
    })?;

    let reply_ok = parsed.get("reply").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    let feedback_ok = parsed.get("feedback").map(|v| !v.is_null()).unwrap_or(false);
    if !reply_ok || !feedback_ok {
        return Err(ai_output_validation_failed());
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Default)]
pub struct SessionSummaryRequest {
    pub messages: Option<Vec<HistoryEntry>>,
    pub scenario: Option<ScenarioInput>,
    pub level: Option<String>,
    pub language: Option<LanguageInput>,
}

pub async fn generate_session_summary(pool: &PgPool, ai: &dyn AIProvider, model: &str, req: SessionSummaryRequest) -> Result<serde_json::Value, AppError> {
    let lang_name = req.language.as_ref().map(|l| l.name.as_str()).unwrap_or("English");
    let default_indonesian = format!("Bahasa {lang_name}");
    let lang_indonesian = req.language.as_ref().and_then(|l| l.indonesian_name.as_deref()).unwrap_or(&default_indonesian);
    let transcript = req.messages.as_deref().unwrap_or(&[]).iter().map(|m| format!("{}: {}", m.role.to_uppercase(), m.content)).collect::<Vec<_>>().join("\n");
    let scenario_title = req.scenario.as_ref().and_then(|s| s.title.as_deref()).unwrap_or("Language Speaking Practice");
    let level = req.level.as_deref().unwrap_or("Intermediate B1");

    let system_prompt = "You are a certified multilingual language assessor and speaking proficiency evaluator.\n\
Respond with ONLY a single JSON object (no markdown fences), matching exactly this shape:\n\
{\"overallScore\": number, \"cefrLevel\": string, \"scores\": {\"fluency\": number, \"grammar\": number, \"vocabulary\": number, \"coherence\": number}, \"strengths\": string[], \"areasToImprove\": [{\"issue\": string, \"advice\": string, \"example\": string}], \"recommendedVocabulary\": [{\"term\": string, \"definition\": string}], \"closingNoteTargetLanguage\"?: string, \"closingNoteEnglish\"?: string, \"closingNoteIndonesian\": string}".to_string();

    let user_prompt = format!(
        "Analyze this {lang_name} ({lang_indonesian}) conversation practice session:\n\
Topic: {scenario_title}\n\
Target Level: {level}\n\
Target Language: {lang_name}\n\n\
Transcript:\n{transcript}\n\n\
Evaluate the student's speaking performance:\n\
1. Overall Speaking Score (0-100)\n\
2. CEFR / Language Scale Assessment (e.g. A2+, B1, B2, HSK 3, TOPIK 2, etc.)\n\
3. Sub-scores (0-100) for: Fluency, Grammar Accuracy, Vocabulary Richness, Pronunciation / Expression\n\
4. Top 3 Strengths demonstrated in {lang_name}\n\
5. Top 3 Key Areas to Improve with concrete actionable tips tailored to Indonesian speakers learning {lang_name}\n\
6. Key Vocabulary & phrases recommended for this topic with Indonesian meanings\n\
7. Encouraging closing feedback in {lang_name} (closingNoteTargetLanguage) and a warm summary in Indonesian (closingNoteIndonesian)"
    );

    let max_tokens = resolve_max_tokens(pool, model, 3000).await;
    let generation = ai
        .generate(GenerationRequest { model: model.to_string(), system_prompt, user_prompt, temperature: 0.5, max_tokens, image_url: None, json_mode: true })
        .await
        .map_err(|e| {
            tracing::warn!(error = ?e, "speaking room session summary generation failed");
            ai_output_validation_failed()
        })?;

    let text = strip_code_fence(&generation.text);
    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        tracing::warn!(error = ?e, text, "speaking room session summary output failed to parse as JSON");
        ai_output_validation_failed()
    })?;

    let score_ok = parsed.get("overallScore").map(|v| v.is_number()).unwrap_or(false);
    let scores_ok = parsed.get("scores").map(|v| !v.is_null()).unwrap_or(false);
    if !score_ok || !scores_ok {
        return Err(ai_output_validation_failed());
    }
    Ok(parsed)
}

// TTS/transcribe are thin passthroughs to the existing AIProvider — no
// new provider code. Unlike generate_turn/generate_session_summary, the
// Bun original does NOT catch a provider failure here — it propagates
// uncaught to the global error handler, which wraps any non-AppError as
// AppError.internal (500), not a 422 — reproduced as-is via
// AppError::Internal rather than mapping to ai_output_validation_failed.
pub async fn synthesize_turn_audio(ai: &dyn AIProvider, model: &str, text: &str, voice: &str) -> Result<SpeechResult, AppError> {
    if text.trim().is_empty() {
        return Err(AppError::UnprocessableEntity("text_required", "text must not be empty".to_string()));
    }
    ai.synthesize_speech(text, voice, model).await.map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}
