use axum::{extract::State, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::services::health;
use crate::state::AppState;

// GET /health
pub async fn get_health(State(state): State<Arc<AppState>>) -> Json<health::HealthStatus> {
    Json(health::check(&state.db, &state.redis).await)
}

// Router fallback for any unmatched path/method — see routes/mod.rs's
// doc comment on why this returns a real {"error","detail"} body
// instead of axum's default empty 404.
pub async fn not_found() -> AppError {
    AppError::NotFound("not_found")
}
