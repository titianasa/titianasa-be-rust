use axum::{extract::State, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::services::live_chat::{self, ChatTurnRequest, ChatTurnResponse};
use crate::state::AppState;

// POST /ai/live-chat-turn — one turn of the Modul Belajar's text tutor.
pub async fn post_live_chat_turn(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<ChatTurnRequest>,
) -> Result<Json<ChatTurnResponse>, AppError> {
    Ok(Json(live_chat::generate_turn(&state.db, state.ai_provider.as_ref(), &state.config.ai_live_chat_model, &ctx, body).await?))
}
