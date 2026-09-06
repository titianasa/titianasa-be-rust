use std::collections::HashMap;
use std::sync::OnceLock;

use tokio::sync::OnceCell;

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

#[async_trait::async_trait]
pub trait AIProvider: Send + Sync {
    async fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, AIProviderError>;
    // ADR-0010 — `model` is passed per-call, same convention as
    // GenerationRequest.model, not baked into the provider instance.
    async fn transcribe(&self, audio: &[u8], mime_type: &str, model: &str) -> Result<TranscriptionResult, AIProviderError>;
    async fn synthesize_speech(&self, text: &str, voice: &str, model: &str) -> Result<SpeechResult, AIProviderError>;
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

// Falls back to `fallback` if the model isn't in OpenRouter's list or the
// lookup itself fails — never fails the caller's generation attempt.
pub async fn resolve_max_tokens(model: &str, fallback: i64) -> i64 {
    match MODEL_MAX_TOKENS.get_or_try_init(fetch_model_max_tokens).await {
        Ok(map) => map.get(model).copied().unwrap_or(fallback),
        Err(e) => {
            tracing::warn!(error = ?e, "resolveMaxTokens: OpenRouter /models lookup failed, using fallback");
            fallback
        }
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
        Self { api_key, client: reqwest::Client::new() }
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

        Ok(GenerationResponse { text, tokens_used: parsed.usage.map(|u| u.total_tokens) })
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
            FakeResult::Ok(text) => Ok(GenerationResponse { text: text.clone(), tokens_used: Some(42) }),
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
