// P39-005 (ADR-0013 L1) — POST /events, titian-web's `lib/telemetry.ts`.

use axum::{extract::State, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::services::client_events::{self, ClientEvent, SubmitEventsResponse};
use crate::state::AppState;

#[derive(Debug, serde::Deserialize)]
pub struct SubmitEventsRequest {
    pub events: Vec<ClientEvent>,
}

// POST /events
pub async fn post_events(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<SubmitEventsRequest>,
) -> Result<Json<SubmitEventsResponse>, AppError> {
    Ok(Json(client_events::submit(&state.db, &state.redis, &ctx, body.events).await?))
}
