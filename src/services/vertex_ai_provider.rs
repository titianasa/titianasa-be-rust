// Vertex AI (Gemini) — the TEXT and VISION half of the AI provider
// split (`generate()`, including OCR/vision via `image_url` — Gemini is
// natively multimodal, no separate vision model needed). STT and TTS
// stay on `DeepSeekProvider` (OpenRouter), which has no Vertex
// equivalent wired up yet — see AppState::text_ai_provider vs
// AppState::ai_provider.
//
// Auth is handled entirely by `gcp_auth`, which tries (in order):
// GOOGLE_APPLICATION_CREDENTIALS, `gcloud auth application-default
// login`'s cached credentials, the Compute Engine metadata server, then
// the `gcloud` CLI itself. On a dev machine that means `gcloud auth
// application-default login` is enough; on a GCE VM with a service
// account attached, no local credentials of any kind are needed. Tokens
// are cached and refreshed internally — never cache one here.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures_util::{Stream, StreamExt};
use gcp_auth::TokenProvider;

use crate::services::ai_provider::{AIProvider, AIProviderError, GenerationRequest, GenerationResponse, SpeechResult, TextChunkStream, TranscriptionResult};

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// The `global` location (needed for newer/preview models not yet
/// rolled out to regional endpoints, e.g. gemini-3.8-flash) drops the
/// region subdomain prefix entirely — `aiplatform.googleapis.com`, not
/// `global-aiplatform.googleapis.com`. Every other location keeps the
/// regional-subdomain form. A free function (not a method) so this
/// string-building is testable without standing up a real TokenProvider.
fn build_endpoint(project_id: &str, region: &str, model: &str) -> String {
    let host = if region == "global" { "aiplatform.googleapis.com".to_string() } else { format!("{region}-aiplatform.googleapis.com") };
    format!("https://{host}/v1/projects/{project_id}/locations/{region}/publishers/google/models/{model}:generateContent")
}

/// Same host rule as `build_endpoint`, `:streamGenerateContent` instead
/// of `:generateContent` — Vertex's incremental-response action, read
/// with `?alt=sse` so the body arrives as Server-Sent Events rather
/// than one huge streamed JSON array.
fn build_stream_endpoint(project_id: &str, region: &str, model: &str) -> String {
    let host = if region == "global" { "aiplatform.googleapis.com".to_string() } else { format!("{region}-aiplatform.googleapis.com") };
    format!("https://{host}/v1/projects/{project_id}/locations/{region}/publishers/google/models/{model}:streamGenerateContent?alt=sse")
}

/// Pulls every COMPLETE SSE event (`data: {...}\n\n`) currently sitting
/// in `buffer`, leaving any trailing partial event for the next chunk of
/// bytes to complete — a single TCP read has no obligation to land on an
/// event boundary. Each complete event is parsed as one `GeminiResponse`
/// chunk; events with no text part (a bare `usageMetadata` tail, a
/// keep-alive) are silently dropped rather than yielded as empty chunks.
/// A free function (not a method) so the framing logic is testable
/// without a real SSE connection.
fn drain_sse_text_events(buffer: &mut String) -> Vec<String> {
    // SSE line endings may be `\n` or `\r\n` — Google's edge uses the
    // latter, so a bare `find("\n\n")` never matches `\r\n\r\n` and every
    // event sits in `buffer` forever, never draining (the bug this
    // normalization fixes: a live-chat stream that opened fine but
    // yielded zero chunks before the connection closed). A JSON string
    // value can't contain a raw, unescaped newline, so blanket-replacing
    // `\r\n` with `\n` across the whole buffer can't corrupt an event's
    // payload.
    if buffer.contains('\r') {
        *buffer = buffer.replace("\r\n", "\n");
    }
    let mut out = Vec::new();
    while let Some(boundary) = buffer.find("\n\n") {
        let event: String = buffer.drain(..boundary + 2).collect();
        for line in event.lines() {
            let Some(data) = line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")) else { continue };
            let Ok(parsed) = serde_json::from_str::<GeminiResponse>(data.trim()) else { continue };
            if let Some(text) = parsed.candidates.into_iter().next().and_then(|c| candidate_text(c.content)) {
                out.push(text);
            }
        }
    }
    out
}

// --- Retrying a busy Vertex ---
//
// gemini-3.8-flash on the `global` endpoint runs on shared capacity:
// under load it answers 429 RESOURCE_EXHAUSTED, or drops the connection
// a minute or more into a request. Both clear up on their own within
// seconds. Retrying at once (what every caller's own loop did) mostly
// hits the same wall: the first pilot run lost a whole generation that
// way, and only survived because the batch script waited and asked
// again. The wait belongs here, where a 429 can still be told apart
// from a reply the model got wrong.

/// Pauses before each retry of a transient failure — three retries,
/// ~46s of waiting in the worst case, before the error reaches the
/// caller.
const TRANSIENT_RETRY_DELAYS: [Duration; 3] = [Duration::from_secs(4), Duration::from_secs(12), Duration::from_secs(30)];

struct Failure {
    error: AIProviderError,
    /// Worth asking again unchanged after a pause.
    transient: bool,
}

impl Failure {
    fn permanent(message: String) -> Self {
        Self { error: AIProviderError(message), transient: false }
    }
}

/// 429 and the 5xx a gateway returns while overloaded. A 400 (bad
/// request, over-long max_tokens) or 403 will fail identically every
/// time, so retrying those only delays the real error.
fn is_transient_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// A dropped or refused connection. A TIMEOUT is not retried: at 240s
/// it is almost certainly a generation too long for the limit, and
/// three more four-minute waits would not change that.
fn is_transient_transport(e: &reqwest::Error) -> bool {
    !e.is_timeout() && (e.is_connect() || e.is_request() || e.is_body())
}

/// `jitter` in [0, 1) spreads the wait ±25%, so several generations
/// that failed together do not all come back at the same instant.
fn backoff_delay(attempt: usize, jitter: f64) -> Duration {
    let base = TRANSIENT_RETRY_DELAYS[attempt.min(TRANSIENT_RETRY_DELAYS.len() - 1)];
    base.mul_f64(0.75 + 0.5 * jitter.clamp(0.0, 1.0))
}

pub struct VertexGeminiProvider {
    project_id: String,
    region: String,
    client: reqwest::Client,
    tokens: Arc<dyn TokenProvider>,
}

impl VertexGeminiProvider {
    /// Discovers credentials at construction time (see module docs) —
    /// fails fast at boot, same convention as `OPENROUTER_API_KEY`/R2 in
    /// `AppState::init`, rather than surfacing as an opaque 500 on the
    /// first request.
    pub async fn new(project_id: String, region: String) -> anyhow::Result<Self> {
        let tokens = gcp_auth::provider().await?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(240))
            .build()
            .expect("reqwest client with static timeout config never fails to build");
        Ok(Self { project_id, region, client, tokens })
    }

    fn endpoint(&self, model: &str) -> String {
        build_endpoint(&self.project_id, &self.region, model)
    }

    /// Gemini's REST API has no notion of "fetch this URL yourself" the
    /// way OpenRouter's OpenAI-compatible `image_url` does — it takes
    /// either inline base64 bytes or a `gs://` Cloud Storage URI. A
    /// signed R2 GET url is neither, so this provider downloads the
    /// bytes itself and inlines them as base64, same request either way
    /// regardless of what storage backend signed the url.
    async fn fetch_inline_image(&self, image_url: &str) -> Result<serde_json::Value, AIProviderError> {
        let response = self.client.get(image_url).send().await.map_err(|e| AIProviderError(format!("failed to fetch image for vision request: {e}")))?;
        if !response.status().is_success() {
            return Err(AIProviderError(format!("failed to fetch image for vision request: HTTP {}", response.status())));
        }
        let mime_type = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("image/jpeg").split(';').next().unwrap_or("image/jpeg").to_string();
        let bytes = response.bytes().await.map_err(|e| AIProviderError(format!("failed to read image bytes: {e}")))?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(serde_json::json!({"inlineData": {"mimeType": mime_type, "data": data}}))
    }

    /// Shared between `generate` and `generate_stream` — the request
    /// body is identical either way, only the endpoint/parsing differs.
    /// One request, with its failure classified. Everything that can be
    /// told apart only here — the HTTP status, whether the connection
    /// dropped — is decided here, before it collapses into a string.
    async fn generate_once(&self, model: &str, body: &serde_json::Value) -> Result<GenerationResponse, Failure> {
        let token = self
            .tokens
            .token(&[CLOUD_PLATFORM_SCOPE])
            .await
            .map_err(|e| Failure::permanent(format!("gcp_auth token fetch failed: {e}")))?;

        let response = self
            .client
            .post(self.endpoint(model))
            .header("authorization", format!("Bearer {}", token.as_str()))
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| Failure { transient: is_transient_transport(&e), error: AIProviderError(e.to_string()) })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(Failure { transient: is_transient_status(status.as_u16()), error: AIProviderError(format!("HTTP {status}: {text}")) });
        }

        let parsed: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Failure { transient: e.is_body(), error: AIProviderError(e.to_string()) })?;

        let candidate = parsed.candidates.into_iter().next();
        let finish_reason = candidate.as_ref().and_then(|c| c.finish_reason.clone());
        // A reply the model itself cut short is not a busy server — the
        // same request would stop the same way, so it is left to the
        // caller's own (output-validating) retry instead.
        ensure_finished(finish_reason.as_deref()).map_err(|error| Failure { error, transient: false })?;
        let text = candidate.and_then(|c| candidate_text(c.content));

        // Gemini reports a truncated response via `finishReason` (e.g.
        // "MAX_TOKENS") rather than an empty `content`, the way
        // OpenRouter's reasoning-model quirk does — both end up here as
        // "no usable text", which is the caller-visible contract this
        // trait already promises (see DeepSeekProvider::generate).
        let text = text.ok_or_else(|| {
            Failure::permanent(format!("empty or missing candidate text in Vertex AI response (finishReason: {})", finish_reason.as_deref().unwrap_or("unknown")))
        })?;

        Ok(GenerationResponse { text, tokens_used: parsed.usage_metadata.and_then(|u| u.total_token_count) })
    }

    async fn build_body(&self, req: &GenerationRequest) -> Result<serde_json::Value, AIProviderError> {
        let mut generation_config = serde_json::json!({
            "temperature": req.temperature,
            "maxOutputTokens": req.max_tokens,
        });
        if req.json_mode {
            generation_config["responseMimeType"] = serde_json::json!("application/json");
        }

        let mut parts = vec![serde_json::json!({"text": req.user_prompt})];
        if let Some(image_url) = &req.image_url {
            parts.push(self.fetch_inline_image(image_url).await?);
        }

        Ok(serde_json::json!({
            "systemInstruction": {"parts": [{"text": req.system_prompt}]},
            "contents": [{"role": "user", "parts": parts}],
            "generationConfig": generation_config,
        }))
    }
}

#[derive(serde::Deserialize)]
struct GeminiPart {
    text: Option<String>,
    /// A thought-summary part — the model's reasoning, never the answer.
    #[serde(default)]
    thought: bool,
}

/// The whole answer text of one candidate. Gemini is free to split a
/// single reply across several `parts`, and a long generation (a full
/// Modul Belajar) does get split — reading only `parts[0]` silently kept
/// the first ~800 characters of a 5-section module and threw the rest
/// away, while the request still counted as a success.
fn candidate_text(content: Option<GeminiContent>) -> Option<String> {
    let text: String = content?.parts.into_iter().filter(|p| !p.thought).filter_map(|p| p.text).collect();
    (!text.is_empty()).then_some(text)
}

/// A reply that stopped for any reason other than finishing is not a
/// shorter answer, it is a broken one — cut off at MAX_TOKENS, or
/// withheld for SAFETY/RECITATION partway through. Every caller of
/// `generate` parses the text as a complete document (JSON, a delimited
/// plan), so passing a fragment on as success only moves the failure
/// somewhere quieter: into the database.
fn ensure_finished(finish_reason: Option<&str>) -> Result<(), AIProviderError> {
    match finish_reason {
        None | Some("STOP") => Ok(()),
        Some(other) => Err(AIProviderError(format!("Vertex AI response did not finish (finishReason: {other})"))),
    }
}

#[derive(serde::Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(serde::Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize)]
struct GeminiUsage {
    #[serde(rename = "totalTokenCount")]
    total_token_count: Option<i64>,
}

#[derive(serde::Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsage>,
}

#[async_trait::async_trait]
impl AIProvider for VertexGeminiProvider {
    async fn generate(&self, req: GenerationRequest) -> Result<GenerationResponse, AIProviderError> {
        let body = self.build_body(&req).await?;
        let mut attempt = 0;
        loop {
            match self.generate_once(&req.model, &body).await {
                Ok(response) => return Ok(response),
                Err(Failure { error, transient: true }) if attempt < TRANSIENT_RETRY_DELAYS.len() => {
                    let delay = backoff_delay(attempt, rand::random::<f64>());
                    tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, error = %error.0, "Vertex AI busy or dropped the connection, retrying after backoff");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(Failure { error, .. }) => return Err(error),
            }
        }
    }

    async fn generate_stream(&self, req: GenerationRequest) -> Result<TextChunkStream, AIProviderError> {
        let token = self.tokens.token(&[CLOUD_PLATFORM_SCOPE]).await.map_err(|e| AIProviderError(format!("gcp_auth token fetch failed: {e}")))?;
        let body = self.build_body(&req).await?;

        let response = self
            .client
            .post(build_stream_endpoint(&self.project_id, &self.region, &req.model))
            .header("authorization", format!("Bearer {}", token.as_str()))
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

        // State threaded through `stream::unfold`: the raw byte stream
        // plus whatever partial (not-yet-a-complete-event) text is still
        // sitting in `buffer` from the last read. Each step first tries
        // to drain an already-buffered event before pulling more bytes,
        // so one read that happens to contain several events doesn't
        // silently drop all but the first.
        struct State {
            bytes: std::pin::Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>,
            buffer: String,
            pending: std::collections::VecDeque<String>,
        }
        let state = State { bytes: Box::pin(response.bytes_stream()), buffer: String::new(), pending: std::collections::VecDeque::new() };

        let stream = futures_util::stream::unfold(state, |mut st| async move {
            loop {
                if let Some(text) = st.pending.pop_front() {
                    return Some((Ok(text), st));
                }
                match st.bytes.next().await {
                    Some(Ok(chunk)) => {
                        st.buffer.push_str(&String::from_utf8_lossy(&chunk));
                        st.pending.extend(drain_sse_text_events(&mut st.buffer));
                        // No complete event yet — loop back and read more
                        // bytes rather than returning early with nothing.
                    }
                    Some(Err(e)) => return Some((Err(AIProviderError(format!("stream read failed: {e}"))), st)),
                    None => return None,
                }
            }
        });

        Ok(Box::pin(stream))
    }

    async fn transcribe(&self, _audio: &[u8], _mime_type: &str, _model: &str) -> Result<TranscriptionResult, AIProviderError> {
        Err(AIProviderError("VertexGeminiProvider does not implement transcribe — STT stays on the OpenRouter provider (AppState::ai_provider)".to_string()))
    }

    async fn synthesize_speech(&self, _text: &str, _voice: &str, _model: &str) -> Result<SpeechResult, AIProviderError> {
        Err(AIProviderError("VertexGeminiProvider does not implement synthesize_speech — TTS stays on the OpenRouter provider (AppState::ai_provider)".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_global_location_drops_the_region_subdomain() {
        let url = build_endpoint("my-project", "global", "gemini-3.8-flash");
        assert_eq!(url, "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global/publishers/google/models/gemini-3.8-flash:generateContent");
    }

    #[test]
    fn a_real_region_keeps_the_regional_subdomain() {
        let url = build_endpoint("my-project", "asia-southeast1", "gemini-2.5-flash");
        assert_eq!(
            url,
            "https://asia-southeast1-aiplatform.googleapis.com/v1/projects/my-project/locations/asia-southeast1/publishers/google/models/gemini-2.5-flash:generateContent"
        );
    }

    #[test]
    fn the_stream_endpoint_uses_alt_sse_and_the_stream_action() {
        let url = build_stream_endpoint("my-project", "global", "gemini-3.8-flash");
        assert_eq!(url, "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global/publishers/google/models/gemini-3.8-flash:streamGenerateContent?alt=sse");
    }

    fn sse_event(text: &str) -> String {
        format!("data: {{\"candidates\":[{{\"content\":{{\"parts\":[{{\"text\":\"{text}\"}}]}}}}]}}\n\n")
    }

    #[test]
    fn a_single_complete_event_is_drained() {
        let mut buffer = sse_event("Hal");
        let texts = drain_sse_text_events(&mut buffer);
        assert_eq!(texts, vec!["Hal".to_string()]);
        assert!(buffer.is_empty(), "a fully-consumed event leaves nothing behind");
    }

    #[test]
    fn crlf_line_endings_are_drained_just_like_bare_lf() {
        // Google's edge sends \r\n, not \n — this is the actual bug that
        // made a live stream open fine and then yield zero chunks: a
        // bare `find("\n\n")` never matches `\r\n\r\n`.
        let mut buffer = sse_event("Hal").replace('\n', "\r\n");
        let texts = drain_sse_text_events(&mut buffer);
        assert_eq!(texts, vec!["Hal".to_string()]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn several_events_in_one_read_are_all_drained_in_order() {
        let mut buffer = format!("{}{}{}", sse_event("Hal"), sse_event("o "), sse_event("dunia"));
        let texts = drain_sse_text_events(&mut buffer);
        assert_eq!(texts, vec!["Hal".to_string(), "o ".to_string(), "dunia".to_string()]);
    }

    #[test]
    fn a_partial_trailing_event_is_left_for_the_next_read() {
        let mut buffer = format!("{}data: {{\"candidates\":[{{\"content\"", sse_event("Hal"));
        let texts = drain_sse_text_events(&mut buffer);
        assert_eq!(texts, vec!["Hal".to_string()]);
        assert_eq!(buffer, "data: {\"candidates\":[{\"content\"", "the incomplete event must survive for the next chunk to complete");
    }

    #[test]
    fn an_event_with_no_text_part_is_silently_dropped_not_yielded_as_empty() {
        // A trailing usageMetadata-only chunk, or a keep-alive comment —
        // neither should surface as a spurious empty chunk to the client.
        let mut buffer = "data: {\"usageMetadata\":{\"totalTokenCount\":42}}\n\n".to_string();
        let texts = drain_sse_text_events(&mut buffer);
        assert!(texts.is_empty());
    }

    #[test]
    fn a_reply_split_across_several_parts_is_read_whole() {
        let parsed: GeminiResponse = serde_json::from_str(
            r#"{"candidates":[{"content":{"parts":[{"text":"Bagian satu. "},{"text":"Bagian dua. "},{"text":"Bagian tiga."}]},"finishReason":"STOP"}]}"#,
        )
        .unwrap();
        let content = parsed.candidates.into_iter().next().unwrap().content;
        assert_eq!(candidate_text(content).as_deref(), Some("Bagian satu. Bagian dua. Bagian tiga."));
    }

    #[test]
    fn thought_parts_never_leak_into_the_answer() {
        let parsed: GeminiResponse = serde_json::from_str(
            r#"{"candidates":[{"content":{"parts":[{"text":"let me plan the sections","thought":true},{"text":"{\"questions\": []}"}]}}]}"#,
        )
        .unwrap();
        let content = parsed.candidates.into_iter().next().unwrap().content;
        assert_eq!(candidate_text(content).as_deref(), Some(r#"{"questions": []}"#));
    }

    #[test]
    fn a_streamed_event_with_several_parts_yields_all_of_them() {
        let mut buffer = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ab\"},{\"text\":\"cd\"}]}}]}\n\n".to_string();
        assert_eq!(drain_sse_text_events(&mut buffer), vec!["abcd".to_string()]);
    }

    #[test]
    fn only_a_clean_stop_counts_as_finished() {
        assert!(ensure_finished(None).is_ok());
        assert!(ensure_finished(Some("STOP")).is_ok());
        for cut_off in ["MAX_TOKENS", "SAFETY", "RECITATION", "OTHER"] {
            let err = ensure_finished(Some(cut_off)).unwrap_err();
            assert!(err.0.contains(cut_off), "{cut_off}: {}", err.0);
        }
    }

    #[test]
    fn only_overload_statuses_are_retried() {
        for status in [429, 500, 502, 503, 504] {
            assert!(is_transient_status(status), "{status}");
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(!is_transient_status(status), "{status} fails the same way every time");
        }
    }

    #[test]
    fn backoff_grows_and_stays_within_its_jitter_band() {
        assert_eq!(backoff_delay(0, 0.5), Duration::from_secs(4));
        assert_eq!(backoff_delay(1, 0.5), Duration::from_secs(12));
        assert_eq!(backoff_delay(2, 0.5), Duration::from_secs(30));
        assert_eq!(backoff_delay(0, 0.0), Duration::from_secs(3));
        assert_eq!(backoff_delay(2, 1.0), Duration::from_secs_f64(37.5));
        // Past the table it holds at the longest pause rather than panicking.
        assert_eq!(backoff_delay(9, 0.5), Duration::from_secs(30));
    }
}
