use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::Response,
    Extension, Json,
};
use base64::Engine as _;
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::speaking_room::{PostSummaryRequest, PostTranscribeRequest, PostTtsRequest, PostTurnRequest};
use crate::services::speaking_room::{self, SessionSummaryRequest, TurnRequest};
use crate::state::AppState;

// P29-001. Deliberately stateless — no ctx/persistence needed beyond
// requiring auth at the route level, matching the "no DB writes" scope
// decision. Every route here is open to ANY authenticated user, no role
// check — the least-gated feature area in the whole codebase.

// POST /speaking-room/turn — returns the AI's raw JSON object untouched
// (camelCase fields), a deliberate exception to the snake_case wire
// convention used everywhere else.
pub async fn post_turn(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<PostTurnRequest>) -> Result<Json<serde_json::Value>, AppError> {
    let req = TurnRequest {
        scenario: body.scenario,
        level: body.level,
        user_message: body.user_message,
        history: body.history,
        tutor_persona: body.tutor_persona,
        mode: body.mode,
        language: body.language,
    };
    let result = speaking_room::generate_turn(state.text_ai_provider.as_ref(), &state.config.ai_speaking_room_text_model, req).await?;
    Ok(Json(result))
}

// POST /speaking-room/summary
pub async fn post_summary(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<PostSummaryRequest>) -> Result<Json<serde_json::Value>, AppError> {
    let req = SessionSummaryRequest { messages: body.messages, scenario: body.scenario, level: body.level, language: body.language };
    let result = speaking_room::generate_session_summary(state.text_ai_provider.as_ref(), &state.config.ai_speaking_room_text_model, req).await?;
    Ok(Json(result))
}

// POST /speaking-room/tts — raw audio bytes, not JSON; the response body
// IS the audio (matches GET /lessons/{id}/speaking-prompt-audio's
// precedent of returning bytes directly).
pub async fn post_tts(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<PostTtsRequest>) -> Result<Response, AppError> {
    let voice = body.voice.unwrap_or_else(|| state.config.ai_tts_default_voice.clone());
    let result = speaking_room::synthesize_turn_audio(state.ai_provider.as_ref(), &state.config.ai_speaking_room_tts_model, &body.text, &voice).await?;
    Ok(Response::builder().status(StatusCode::OK).header(header::CONTENT_TYPE, result.content_type).body(Body::from(result.bytes)).unwrap())
}

// POST /speaking-room/transcribe — no service-layer wrapper in the Bun
// original either; a provider failure here propagates uncaught to a 500
// (AppError::Internal), same as post_tts's synthesize failure.
pub async fn post_transcribe(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<PostTranscribeRequest>) -> Result<Json<serde_json::Value>, AppError> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(&body.audio_base64).map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let mime_type = body.mime_type.as_deref().unwrap_or("audio/webm");
    let result = state.ai_provider.transcribe(&bytes, mime_type, &state.config.ai_stt_model).await.map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(Json(serde_json::json!({"transcript": result.text})))
}
