use axum::{routing::post, Router};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// P29-001 — all 4 routes require auth (any authenticated user, no role
// check) but nothing else; none are public.
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/speaking-room/turn", post(handlers::speaking_room::post_turn))
        .route("/speaking-room/summary", post(handlers::speaking_room::post_summary))
        .route("/speaking-room/tts", post(handlers::speaking_room::post_tts))
        .route("/speaking-room/transcribe", post(handlers::speaking_room::post_transcribe))
}
