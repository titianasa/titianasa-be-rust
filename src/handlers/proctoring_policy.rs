use axum::{extract::State, http::StatusCode, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::proctoring::CreatePolicyRequest;
use crate::services::proctoring_policy::{self, PolicyResponse};
use crate::state::AppState;

// POST /proctoring-policies
pub async fn post_policy(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<CreatePolicyRequest>) -> Result<(StatusCode, Json<PolicyResponse>), AppError> {
    let result = proctoring_policy::create_policy(
        &state.db,
        &ctx,
        &body.exam_type,
        body.camera.as_deref().unwrap_or("off"),
        body.microphone.as_deref().unwrap_or("off"),
        body.screen.as_deref().unwrap_or("off"),
        body.fullscreen_required.unwrap_or(false),
        body.focus_monitoring.unwrap_or(false),
        body.retention_days.unwrap_or(30),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

#[derive(serde::Serialize)]
pub struct PolicyListResponse {
    pub items: Vec<PolicyResponse>,
}

// GET /proctoring-policies
pub async fn get_policies(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>) -> Result<Json<PolicyListResponse>, AppError> {
    let items = proctoring_policy::list_policies(&state.db).await?;
    Ok(Json(PolicyListResponse { items }))
}
