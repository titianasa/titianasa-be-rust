use axum::{extract::State, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::services::alm_generation::{self, GenerateFragmentRequest, GenerateFragmentResponse};
use crate::state::AppState;

// POST /ai/generate-alm-fragment — the ALM editor's right-click "AI Asisten".
pub async fn post_generate_fragment(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<GenerateFragmentRequest>,
) -> Result<Json<GenerateFragmentResponse>, AppError> {
    Ok(Json(alm_generation::generate_fragment(&state.db, state.ai_provider.as_ref(), &state.config.ai_lesson_generation_model, &ctx, body).await?))
}
