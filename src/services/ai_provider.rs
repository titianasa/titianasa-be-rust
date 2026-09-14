use std::collections::HashMap;
use std::sync::OnceLock;

use sqlx::PgPool;
use tokio::sync::OnceCell;

use crate::errors::AppError;

// Port of ai_provider.ts. Provider abstraction (ADR-0004) — business
// logic never calls a provider API directly, always through this trait,
// injected via AppState (same pattern as PaymentProvider).

#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub model: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub temperature: f64,
    pub max_tokens: i64,
    /// Gemini "thinking" tokens are billed against `max_tokens`, so a
    /// generous-looking budget can still be spent entirely on reasoning
    /// and return `finishReason: MAX_TOKENS` with no answer at all (seen
    /// on a 3-question quiz batch at 4700). `Some(n)` caps it; `None`
    /// leaves the model's default. Providers that have no such knob
    /// ignore it.
    pub thinking_budget: Option<i64>,
    /// The caller can use a reply the output ceiling cut short, and will
    /// continue it itself (lesson_plan_ai::generate_plan keeps every
    /// section that closed and asks only for the rest). Everyone else
    /// keeps the old contract: a cut-off reply is an error, because
    /// half a JSON array is worse than no answer.
    pub allow_partial: bool,
    // P2-015 (OCR-to-Question) — a signed, publicly-fetchable URL, not
    // raw bytes; unused by R8 callers but kept on the shared request
    // shape for whichever later phase ports OCR.
    pub image_url: Option<String>,
    // P29-002 finding: deepseek/deepseek-v4-flash-latest occasionally
    // leaks its raw chain-of-thought reasoning INTO `content` itself.
    // OpenRouter's response_format:{type:"json_object"} keeps that
    // reasoning out of `content`. Only set this when the expected output
    // is a JSON *object* — it silently collapses a requested JSON
    // *array* into an object, so an array-shaped caller must never set
    // it (none of R8's callers are array-shaped).
    pub json_mode: bool,
}

#[derive(Debug, Clone)]
pub struct GenerationResponse {
    pub text: String,
    pub tokens_used: Option<i64>,
    /// The model stopped at its output ceiling (only ever true when the
    /// request set `allow_partial`).
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("AI provider error: {0}")]
pub struct AIProviderError(pub String);

#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SpeechResult {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// A live sequence of text deltas, e.g. one Gemini `streamGenerateContent`
/// chunk at a time — Live AI Chat forwards each one straight to the
/// browser as it arrives instead of waiting for the whole reply.
pub type TextChunkStream = std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<String, AIProviderError>> + Send>>;

#[async_trait::async_trait]
pub trait AIProvider: Send + Sync {
    async fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, AIProviderError>;
    // ADR-0010 — `model` is passed per-call, same convention as
    // GenerationRequest.model, not baked into the provider instance.
    async fn transcribe(&self, audio: &[u8], mime_type: &str, model: &str) -> Result<TranscriptionResult, AIProviderError>;
    async fn synthesize_speech(&self, text: &str, voice: &str, model: &str) -> Result<SpeechResult, AIProviderError>;
    /// Incremental generation. Default falls back to one non-streaming
    /// `generate()` call, yielded as a single chunk — real
    /// token-by-token streaming is opt-in per provider (currently only
    /// `VertexGeminiProvider`), so this trait doesn't force every
    /// implementor (including the two test doubles) to have one.
    async fn generate_stream(&self, req: GenerationRequest) -> Result<TextChunkStream, AIProviderError> {
        let resp = self.generate(req).await?;
        Ok(Box::pin(futures_util::stream::once(async move { Ok(resp.text) })))
    }
}

// Models don't reliably follow "no markdown fences" instructions — strip
// a leading ```` ```json ` / trailing ` ``` ```` wrapper (and surrounding
// whitespace) if present. Whatever's underneath still gets fully
// validated afterward by the caller.
pub fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let after_open = &trimmed[3..];
    let after_open = after_open.strip_prefix("json").unwrap_or(after_open);
    let after_open = after_open.trim_start_matches(['\n', '\r']);
    let without_close = after_open.strip_suffix("```").unwrap_or(after_open);
    without_close.trim().to_string()
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// P40-003 — the author-facing allowlist (`GET /ai/models`) is now the
/// AI model catalog's `text`-capable, enabled rows
/// (`ai_settings::text_capable_models`), not a hardcoded const — an
/// admin disabling a model in Pengaturan AI removes it from this list
/// on the next call, no deploy needed. `role`'s resolved default
/// (`ai_settings::resolve`) is what `requested: None` falls through to,
/// same "operator's own config isn't second-guessed, only what an
/// author can request" contract as before this ticket.
pub async fn resolve_ai_model(pool: &PgPool, config: &crate::Config, role: &str, requested: Option<&str>) -> Result<String, AppError> {
    match requested {
        None => Ok(crate::services::ai_settings::resolve(pool, config, role).await?.model_id),
        Some(model) => {
            let options = crate::services::ai_settings::text_capable_models(pool).await?;
            if options.iter().any(|c| c.model_id == model) {
                Ok(model.to_string())
            } else {
                Err(AppError::UnprocessableEntity("model_not_allowed", format!("model \"{model}\" tidak diizinkan")))
            }
        }
    }
}

// P29-001 finding: a hardcoded maxTokens guess was found live to be far
// too small for a reasoning-capable model that can burn its entire
// budget on hidden reasoning before writing any visible answer.
// OpenRouter's /models endpoint reports each model's real
// max_completion_tokens; asking for anywhere near that costs nothing
// extra since OpenRouter only bills for tokens actually produced.
// Cached in-process (this list barely changes) rather than fetched every
// call — `OnceCell::get_or_try_init` dedupes concurrent first-callers
// into one fetch AND never caches a failure (an Err leaves the cell
// uninitialized, so the next call retries), matching the Bun
// implementation's `modelMaxTokensFetch`/`modelMaxTokensCache` pair
// exactly.
static MODEL_MAX_TOKENS: OnceCell<HashMap<String, i64>> = OnceCell::const_new();

#[derive(serde::Deserialize)]
struct OpenRouterTopProvider {
    max_completion_tokens: Option<i64>,
}

#[derive(serde::Deserialize)]
struct OpenRouterModel {
    id: String,
    top_provider: Option<OpenRouterTopProvider>,
}

#[derive(serde::Deserialize)]
struct OpenRouterModelsResponse {
    data: Option<Vec<OpenRouterModel>>,
}

async fn fetch_model_max_tokens() -> anyhow::Result<HashMap<String, i64>> {
    let res = http_client().get("https://openrouter.ai/api/v1/models").send().await?;
    if !res.status().is_success() {
        anyhow::bail!("OpenRouter /models HTTP {}", res.status());
    }
    let json: OpenRouterModelsResponse = res.json().await?;
    let mut map = HashMap::new();
    for m in json.data.unwrap_or_default() {
        if let Some(max) = m.top_provider.and_then(|tp| tp.max_completion_tokens) {
            if max > 0 {
                map.insert(m.id, max);
            }
        }
    }
    Ok(map)
}

// Capped at `fallback`, not just floored by it: `fallback` is each
// caller's own estimate of how much output THIS task could ever need
// (900 for a live-chat turn, 32_000 for a full lesson plan). The
// model's own ceiling (e.g. 384_000 for deepseek-v4.1-flash) is an
// upper bound on what the model CAN return, not a hint about what a
// short task SHOULD request — asking for it anyway costs nothing per
// se, but risks tripping a provider-side budget/quota check for no
// benefit. `min` keeps every call's own accounting honest.
//
// Pulled out of `resolve_max_tokens` as its own pure function so this
// decision is unit-testable without a real OpenRouter fetch — only the
// network+cache half below is not worth mocking.
fn capped_max_tokens(model_max: Option<i64>, fallback: i64) -> i64 {
    model_max.map_or(fallback, |model_max| model_max.min(fallback))
}

// P40-003 — a model's ceiling now comes from `ai_model_catalog.max_output_tokens`
// (`ai_settings::model_max_tokens`, in-process cached same as this
// file's own OpenRouter lookup) instead of a hardcoded Vertex-only
// const. This is what actually fixed the bug the old const's own doc
// comment described (Vertex ids never resolving against OpenRouter's
// catalogue) — the seed migration gives `gemini-3.8-flash` the exact
// same 65,536 ceiling the old const did, so behavior is unchanged
// until an admin edits the catalog.
//
// Falls back to `fallback` if the model isn't in the catalog OR
// OpenRouter's list, or any lookup fails — never fails the caller's
// generation attempt.
pub async fn resolve_max_tokens(pool: &PgPool, model: &str, fallback: i64) -> i64 {
    match crate::services::ai_settings::model_max_tokens(pool, model).await {
        Ok(Some(model_max)) => return capped_max_tokens(Some(model_max), fallback),
        Ok(None) => {}
        Err(e) => tracing::warn!(error = ?e, "resolveMaxTokens: catalog lookup failed, falling through to OpenRouter"),
    }
    match MODEL_MAX_TOKENS.get_or_try_init(fetch_model_max_tokens).await {
        Ok(map) => capped_max_tokens(map.get(model).copied(), fallback),
        Err(e) => {
            tracing::warn!(error = ?e, "resolveMaxTokens: OpenRouter /models lookup failed, using fallback");
            fallback
        }
    }
}

/// P40-003 — a role's `ResolvedModel.fallback_model_id` (Pengaturan AI):
/// tries `resolved.model_id` first; if that fails AND a fallback is
/// configured, retries ONCE against the fallback model. Returns which
/// model id actually produced the result, so a caller's `ai_tasks.model`
/// records reality (ADR-0013/ADR-0014: "ai_tasks.model mencatat model
/// yang benar-benar dipakai") instead of always the configured primary.
///
/// Deliberately generic over the request rather than a specific
/// generation function — every one of this codebase's ~16 generation
/// call sites already has its OWN retry-then-validate shape (JSON
/// parsing, empty-reply checks, ...), so this is infrastructure a call
/// site opts into around its EXISTING primary-model attempt, not a
/// replacement for it. Not yet wired into a live call site in this
/// ticket — the DoD itself only requires resolving the CONFIGURED
/// model, not proving the failure path end-to-end; wiring this into a
/// generator is a following ticket's job, one call site at a time.
///
/// No fallback configured → behaves exactly like calling
/// `ai.generate()` directly: the primary's own error is returned
/// untouched, `resolved.model_id` reported as what was "used" (it's the
/// only one that was tried).
pub async fn generate_with_fallback(
    ai: &dyn AIProvider,
    resolved: &crate::services::ai_settings::ResolvedModel,
    request: GenerationRequest,
) -> (Result<GenerationResponse, AIProviderError>, String) {
    let primary_request = GenerationRequest { model: resolved.model_id.clone(), ..request.clone() };
    let primary_result = ai.generate(primary_request).await;
    if primary_result.is_ok() {
        return (primary_result, resolved.model_id.clone());
    }
    let Some(fallback_id) = &resolved.fallback_model_id else {
        return (primary_result, resolved.model_id.clone());
    };
    tracing::warn!(primary_model = %resolved.model_id, fallback_model = %fallback_id, "primary model failed, trying fallback");
    let fallback_request = GenerationRequest { model: fallback_id.clone(), ..request };
    match ai.generate(fallback_request).await {
        Ok(resp) => (Ok(resp), fallback_id.clone()),
        // Both failed — report the PRIMARY's error (the one the caller
        // configured as their real choice), not the fallback's, same
        // "report the meaningful failure" convention this codebase's
        // existing 2-attempt retry loops already follow.
        Err(_) => (primary_result, resolved.model_id.clone()),
    }
}

// ADR-0004's routing table names this provider "deepseek" for every MVP
// task. The only AI credential available is an OpenRouter key, not a
// direct DeepSeek API key — OpenRouter is an OpenAI-compatible router
// that can target DeepSeek's models by id, so this struct calls
// OpenRouter's chat completions endpoint with a DeepSeek model id.
pub struct DeepSeekProvider {
    api_key: String,
    client: reqwest::Client,
}

impl DeepSeekProvider {
    pub fn new(api_key: String) -> Self {
        // No timeout at all previously — a stalled OpenRouter connection
        // hung the request (and the caller's spinner) forever. 240s
        // comfortably covers the slowest real call today (a full-module
        // generation, documented as "1-3 minutes" to the author) with
        // room to spare; connect_timeout is separate and much shorter
        // since a dead connection should fail fast, not eat into the
        // generation budget.
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(240))
            .build()
            .expect("reqwest client with static timeout config never fails to build");
        Self { api_key, client }
    }
}

#[derive(serde::Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(serde::Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(serde::Deserialize)]
struct ChatUsage {
    total_tokens: i64,
}

#[derive(serde::Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[async_trait::async_trait]
impl AIProvider for DeepSeekProvider {
    async fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, AIProviderError> {
        let user_content = match &req.image_url {
            Some(image_url) => serde_json::json!([
                {"type": "text", "text": req.user_prompt},
                {"type": "image_url", "image_url": {"url": image_url}},
            ]),
            None => serde_json::Value::String(req.user_prompt.clone()),
        };

        let mut body = serde_json::json!({
            "model": req.model,
            "messages": [
                {"role": "system", "content": req.system_prompt},
                {"role": "user", "content": user_content},
            ],
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
        });
        if req.json_mode {
            body["response_format"] = serde_json::json!({"type": "json_object"});
        }

        let response = self
            .client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AIProviderError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AIProviderError(format!("HTTP {status}: {text}")));
        }

        let parsed: ChatCompletionResponse = response.json().await.map_err(|e| AIProviderError(e.to_string()))?;

        // `content` can be `null` (not just a missing `choices[0]`) — a
        // reasoning-capable model that spent its whole max_tokens budget
        // on internal reasoning and left no room for the actual answer.
        let text = parsed.choices.into_iter().next().and_then(|c| c.message.content);
        let text = text.ok_or_else(|| AIProviderError("empty or null completion content in provider response".to_string()))?;

        Ok(GenerationResponse { text, tokens_used: parsed.usage.map(|u| u.total_tokens), truncated: false })
    }

    // ADR-0010 — OpenRouter's transcription endpoint, OpenAI-compatible
    // multipart/form-data (not JSON+base64 — simpler for bytes already
    // in memory).
    async fn transcribe(&self, audio: &[u8], mime_type: &str, model: &str) -> Result<TranscriptionResult, AIProviderError> {
        let extension = mime_type.split('/').nth(1).unwrap_or("bin");
        let part = reqwest::multipart::Part::bytes(audio.to_vec())
            .file_name(format!("audio.{extension}"))
            .mime_str(mime_type)
            .map_err(|e| AIProviderError(e.to_string()))?;
        let form = reqwest::multipart::Form::new().part("file", part).text("model", model.to_string());

        let response = self
            .client
            .post("https://openrouter.ai/api/v1/audio/transcriptions")
            .header("authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await
            .map_err(|e| AIProviderError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AIProviderError(format!("HTTP {status}: {text}")));
        }

        #[derive(serde::Deserialize)]
        struct TranscribeResponse {
            text: String,
        }
        let parsed: TranscribeResponse = response.json().await.map_err(|e| AIProviderError(e.to_string()))?;
        Ok(TranscriptionResult { text: parsed.text })
    }

    // ADR-0010 — OpenRouter's speech endpoint returns raw audio bytes
    // (not JSON) on success. P29-001: Gemini's TTS models reject
    // response_format="mp3" outright, unlike Kokoro — PCM has no header
    // a browser <audio> element can use to know sample rate/channels, so
    // it's wrapped into a WAV container here (Gemini's documented output
    // is 24kHz/16-bit/mono).
    async fn synthesize_speech(&self, text: &str, voice: &str, model: &str) -> Result<SpeechResult, AIProviderError> {
        let is_gemini_tts = model.starts_with("google/gemini") && model.contains("tts");
        let body = serde_json::json!({
            "model": model,
            "input": text,
            "voice": voice,
            "response_format": if is_gemini_tts { "pcm" } else { "mp3" },
        });

        let response = self
            .client
            .post("https://openrouter.ai/api/v1/audio/speech")
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AIProviderError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AIProviderError(format!("HTTP {status}: {text}")));
        }

        if is_gemini_tts {
            let pcm = response.bytes().await.map_err(|e| AIProviderError(e.to_string()))?;
            return Ok(SpeechResult { bytes: pcm_to_wav(&pcm, 24000, 1, 16), content_type: "audio/wav".to_string() });
        }

        let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("audio/mpeg").to_string();
        let bytes = response.bytes().await.map_err(|e| AIProviderError(e.to_string()))?.to_vec();
        Ok(SpeechResult { bytes, content_type })
    }
}

// Wraps raw 16-bit mono PCM into a playable WAV file — ported from
// speaking-main/server.ts's pcmToWav. Gemini TTS's documented output is
// 24kHz/16-bit/mono; not configurable per-call via this endpoint.
fn pcm_to_wav(pcm: &[u8], sample_rate: u32, num_channels: u16, bits_per_sample: u16) -> Vec<u8> {
    let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
    let block_align = num_channels * bits_per_sample / 8;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&num_channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

// Test double — returns a preset response (or error) instead of making a
// network call, mirroring FakeAIProvider in ai_provider.ts. Public (not
// #[cfg(test)]) so integration tests in tests/*.rs can build an AppState
// with it, same as StubQrisProvider.
enum FakeResult {
    Ok(String),
    Err(String),
}

pub struct FakeAIProvider {
    generate_result: FakeResult,
    // Independent of generate_result: a speaking-evaluation test needs
    // to fail transcribe() specifically while generate() still succeeds
    // (or the reverse).
    transcribe_result: FakeResult,
}

impl FakeAIProvider {
    pub fn success(text: impl Into<String>) -> Self {
        Self { generate_result: FakeResult::Ok(text.into()), transcribe_result: FakeResult::Ok("fake transcript".into()) }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self { generate_result: FakeResult::Err(message.into()), transcribe_result: FakeResult::Ok("fake transcript".into()) }
    }

    pub fn with_transcription(mut self, text: impl Into<String>) -> Self {
        self.transcribe_result = FakeResult::Ok(text.into());
        self
    }

    pub fn with_transcription_failure(mut self, message: impl Into<String>) -> Self {
        self.transcribe_result = FakeResult::Err(message.into());
        self
    }
}

#[async_trait::async_trait]
impl AIProvider for FakeAIProvider {
    async fn generate(&self, _req: GenerationRequest) -> Result<GenerationResponse, AIProviderError> {
        match &self.generate_result {
            FakeResult::Ok(text) => Ok(GenerationResponse { text: text.clone(), tokens_used: Some(42), truncated: false }),
            FakeResult::Err(msg) => Err(AIProviderError(msg.clone())),
        }
    }

    async fn transcribe(&self, _audio: &[u8], _mime_type: &str, _model: &str) -> Result<TranscriptionResult, AIProviderError> {
        match &self.transcribe_result {
            FakeResult::Ok(text) => Ok(TranscriptionResult { text: text.clone() }),
            FakeResult::Err(msg) => Err(AIProviderError(msg.clone())),
        }
    }

    // No failure mode yet — nothing in this codebase tests a TTS-failure
    // path; add one if/when a real caller needs it.
    async fn synthesize_speech(&self, _text: &str, _voice: &str, _model: &str) -> Result<SpeechResult, AIProviderError> {
        Ok(SpeechResult { bytes: vec![0, 1, 2, 3], content_type: "audio/mpeg".to_string() })
    }
}

/// Test double for Phase 38's "retry once" behavior: `generate()` fails
/// the first `fail_times` calls, then returns `success_text` from then
/// on — so a caller's retry loop can be proven to clear exactly that
/// much flakiness (and no more). An atomic counter, not a `Cell`,
/// because `AIProvider` is `Send + Sync` and called through `&dyn`
/// across await points. Public (not `#[cfg(test)]`) for the same reason
/// `FakeAIProvider` is — integration tests in `tests/*.rs` build an
/// `AppState` with it directly.
pub struct FlakyThenSuccessProvider {
    fail_times: usize,
    calls_so_far: std::sync::atomic::AtomicUsize,
    success_text: String,
}

impl FlakyThenSuccessProvider {
    pub fn new(fail_times: usize, success_text: impl Into<String>) -> Self {
        Self { fail_times, calls_so_far: std::sync::atomic::AtomicUsize::new(0), success_text: success_text.into() }
    }
}

#[async_trait::async_trait]
impl AIProvider for FlakyThenSuccessProvider {
    async fn generate(&self, _req: GenerationRequest) -> Result<GenerationResponse, AIProviderError> {
        let call_index = self.calls_so_far.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call_index < self.fail_times {
            Err(AIProviderError("simulated transient provider failure".to_string()))
        } else {
            Ok(GenerationResponse { text: self.success_text.clone(), tokens_used: Some(7), truncated: false })
        }
    }

    async fn transcribe(&self, _audio: &[u8], _mime_type: &str, _model: &str) -> Result<TranscriptionResult, AIProviderError> {
        Ok(TranscriptionResult { text: "unused".to_string() })
    }

    async fn synthesize_speech(&self, _text: &str, _voice: &str, _model: &str) -> Result<SpeechResult, AIProviderError> {
        Ok(SpeechResult { bytes: vec![], content_type: "audio/mpeg".to_string() })
    }
}

/// Answers each call with the next reply in `replies` (the last one
/// repeats) and keeps every user prompt it was sent, so a test can
/// check both what came back and what a follow-up call ASKED for — the
/// continuation of a cut-off article is only correct if it asks for the
/// missing sections and not the whole article again.
pub struct ScriptedProvider {
    replies: Vec<String>,
    calls: std::sync::Mutex<Vec<String>>,
}

impl ScriptedProvider {
    pub fn new(replies: Vec<String>) -> Self {
        Self { replies, calls: std::sync::Mutex::new(Vec::new()) }
    }

    pub fn prompts(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AIProvider for ScriptedProvider {
    async fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, AIProviderError> {
        let mut calls = self.calls.lock().unwrap();
        let index = calls.len().min(self.replies.len().saturating_sub(1));
        calls.push(req.user_prompt);
        Ok(GenerationResponse { text: self.replies[index].clone(), tokens_used: Some(10), truncated: false })
    }

    async fn transcribe(&self, _audio: &[u8], _mime_type: &str, _model: &str) -> Result<TranscriptionResult, AIProviderError> {
        Ok(TranscriptionResult { text: "unused".to_string() })
    }

    async fn synthesize_speech(&self, _text: &str, _voice: &str, _model: &str) -> Result<SpeechResult, AIProviderError> {
        Ok(SpeechResult { bytes: vec![], content_type: "audio/mpeg".to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fails when `req.model` matches `fail_for`, succeeds (echoing the
    // model id back as the response text, so a test can tell which
    // model actually answered) otherwise — `generate_with_fallback`'s
    // own tests need per-MODEL behavior, unlike `FakeAIProvider`'s
    // single fixed outcome.
    struct ModelAwareProvider {
        fail_for: &'static str,
    }

    #[async_trait::async_trait]
    impl AIProvider for ModelAwareProvider {
        async fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, AIProviderError> {
            if req.model == self.fail_for {
                Err(AIProviderError(format!("{} is down", req.model)))
            } else {
                Ok(GenerationResponse { text: req.model, tokens_used: Some(1), truncated: false })
            }
        }
        async fn transcribe(&self, _audio: &[u8], _mime_type: &str, _model: &str) -> Result<TranscriptionResult, AIProviderError> {
            unimplemented!()
        }
        async fn synthesize_speech(&self, _text: &str, _voice: &str, _model: &str) -> Result<SpeechResult, AIProviderError> {
            unimplemented!()
        }
    }

    fn fallback_test_request() -> GenerationRequest {
        GenerationRequest { model: String::new(), system_prompt: "sp".into(), user_prompt: "up".into(), temperature: 0.0, max_tokens: 100, image_url: None, json_mode: false, thinking_budget: None, allow_partial: false }
    }

    #[tokio::test]
    async fn primary_success_never_touches_the_fallback() {
        let ai = ModelAwareProvider { fail_for: "never-used" };
        let resolved = crate::services::ai_settings::ResolvedModel { model_id: "primary".into(), fallback_model_id: Some("fallback".into()), temperature: None, max_tokens: None };
        let (result, used) = generate_with_fallback(&ai, &resolved, fallback_test_request()).await;
        assert_eq!(result.unwrap().text, "primary");
        assert_eq!(used, "primary", "ai_tasks.model must record the model that actually answered");
    }

    #[tokio::test]
    async fn primary_failure_falls_through_to_the_fallback_and_reports_it_as_used() {
        let ai = ModelAwareProvider { fail_for: "primary" };
        let resolved = crate::services::ai_settings::ResolvedModel { model_id: "primary".into(), fallback_model_id: Some("fallback".into()), temperature: None, max_tokens: None };
        let (result, used) = generate_with_fallback(&ai, &resolved, fallback_test_request()).await;
        assert_eq!(result.unwrap().text, "fallback");
        assert_eq!(used, "fallback");
    }

    #[tokio::test]
    async fn no_fallback_configured_just_returns_the_primarys_own_error() {
        let ai = ModelAwareProvider { fail_for: "primary" };
        let resolved = crate::services::ai_settings::ResolvedModel { model_id: "primary".into(), fallback_model_id: None, temperature: None, max_tokens: None };
        let (result, used) = generate_with_fallback(&ai, &resolved, fallback_test_request()).await;
        assert!(result.is_err());
        assert_eq!(used, "primary");
    }

    struct AlwaysFails;
    #[async_trait::async_trait]
    impl AIProvider for AlwaysFails {
        async fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, AIProviderError> {
            Err(AIProviderError(format!("{} is down", req.model)))
        }
        async fn transcribe(&self, _audio: &[u8], _mime_type: &str, _model: &str) -> Result<TranscriptionResult, AIProviderError> {
            unimplemented!()
        }
        async fn synthesize_speech(&self, _text: &str, _voice: &str, _model: &str) -> Result<SpeechResult, AIProviderError> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn both_primary_and_fallback_failing_reports_the_primarys_error() {
        let ai = AlwaysFails;
        let resolved = crate::services::ai_settings::ResolvedModel { model_id: "primary".into(), fallback_model_id: Some("fallback".into()), temperature: None, max_tokens: None };
        let (result, used) = generate_with_fallback(&ai, &resolved, fallback_test_request()).await;
        assert_eq!(result.unwrap_err().0, "primary is down", "the PRIMARY's error is what's surfaced, not the fallback's");
        assert_eq!(used, "primary");
    }

    #[test]
    fn capped_max_tokens_prefers_the_smaller_of_the_two() {
        // The model can return far more than this task ever needs —
        // stay capped at the caller's own estimate.
        assert_eq!(capped_max_tokens(Some(384_000), 900), 900);
        // The model's real ceiling is smaller than what was asked for —
        // that's the actual limit now, not the caller's optimistic guess.
        assert_eq!(capped_max_tokens(Some(500), 32_000), 500);
        // Model not found in OpenRouter's list — fall through untouched.
        assert_eq!(capped_max_tokens(None, 16_000), 16_000);
    }

    // P40-003 — the ceiling itself now lives in `ai_model_catalog`
    // (seed migration gives `gemini-3.8-flash` 65_536, proven against a
    // real DB by `ai_settings_test.rs::resolve_max_tokens_...`), not a
    // const this file can unit-test in isolation. What's still pure and
    // worth a plain unit test is the CLAMPING math itself — Vertex
    // rejects anything higher than 65_536 for this model with
    // "supported range is from 1 (inclusive) to 65537 (exclusive)".
    #[test]
    fn an_article_asking_for_the_whole_budget_is_clamped_to_the_models_ceiling() {
        let ceiling = Some(65_536);
        assert_eq!(capped_max_tokens(ceiling, 65_536), 65_536);
        assert_eq!(capped_max_tokens(ceiling, 1_000_000), 65_536);
    }
}
