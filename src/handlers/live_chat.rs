use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{extract::State, Extension, Json};
use futures_util::Stream;
use std::convert::Infallible;
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::services::live_chat::{self, ChatTurnRequest, ChatTurnResponse};
use crate::state::AppState;

// POST /ai/live-chat-turn — one turn of the Modul Belajar's text tutor.
// Kept alongside the streaming variant below rather than removed — a
// non-streaming caller (an older client build, an internal script) still
// gets a normal JSON response.
pub async fn post_live_chat_turn(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<ChatTurnRequest>,
) -> Result<Json<ChatTurnResponse>, AppError> {
    Ok(Json(live_chat::generate_turn(&state.db, state.text_ai_provider.as_ref(), &state.config.ai_live_chat_model, &ctx, body).await?))
}

// POST /ai/live-chat-turn/stream — same turn, forwarded to the browser
// as Server-Sent Events one text delta at a time instead of one big
// reply at the end. Event names: "chunk" (data = one text delta),
// "done" (reply finished normally), "error" (provider failed — check
// whether any "chunk" events arrived first: if so, the client already
// has a partial reply worth keeping, not one to discard).
pub async fn post_live_chat_turn_stream(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<ChatTurnRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let stream = live_chat::generate_turn_stream(state.db.clone(), state.text_ai_provider.clone(), state.config.ai_live_chat_model.clone(), ctx, body).await?;
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
