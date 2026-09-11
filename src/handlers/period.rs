use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::period::CreatePeriodRequest;
use crate::services::period::{self, PeriodListResponse, PeriodResponse};
use crate::state::AppState;

// POST /organizations/{id}/periods
pub async fn post_period(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(organization_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreatePeriodRequest>,
) -> Result<(StatusCode, Json<PeriodResponse>), AppError> {
    let result = period::create(&state.db, &ctx, organization_id, body).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /organizations/{id}/periods
pub async fn get_periods(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<PeriodListResponse>, AppError> {
    Ok(Json(period::list_for_org(&state.db, &ctx, organization_id).await?))
}

#[derive(Debug, serde::Deserialize)]
pub struct SetPeriodStatusRequest {
    pub status: String,
}

// PATCH /periods/{id}
pub async fn patch_period(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<SetPeriodStatusRequest>,
) -> Result<Json<PeriodResponse>, AppError> {
    if body.status != "active" && body.status != "archived" {
        return Err(AppError::UnprocessableEntity("invalid_status", "status must be active or archived".to_string()));
    }
    Ok(Json(period::set_status(&state.db, &ctx, id, &body.status).await?))
}
