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
use crate::models::requests::marketplace::CreateCohortRequest;
use crate::services::cohort::{self, CohortListResponse, CohortResponse};
use crate::state::AppState;

// POST /products/{id}/cohorts
pub async fn post_cohort(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(product_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateCohortRequest>,
) -> Result<(StatusCode, Json<CohortResponse>), AppError> {
    let schedule = body.schedule.unwrap_or_else(|| serde_json::json!({}));
    let result = cohort::create_cohort(&state.db, &ctx, product_id, &body.name, &schedule, body.starts_at, body.ends_at, body.meeting_url.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /products/{id}/cohorts
pub async fn get_cohorts(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(product_id): Path<Uuid>,
) -> Result<Json<CohortListResponse>, AppError> {
    Ok(Json(cohort::list_cohorts_for_product(&state.db, &ctx, product_id).await?))
}
