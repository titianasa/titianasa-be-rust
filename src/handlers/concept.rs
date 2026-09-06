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
use crate::models::requests::learning::AddPrerequisiteRequest;
use crate::models::responses::concept::{PrerequisiteEdgeResponse, PrerequisiteItem, PrerequisiteListResponse};
use crate::services::concept;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::state::AppState;

// GET /concepts/{id}/prerequisites
pub async fn get_prerequisites(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(concept_id): Path<Uuid>,
) -> Result<Json<PrerequisiteListResponse>, AppError> {
    require_permission(&ctx, Resource::Module, Action::Create)?;
    let items = concept::list_prerequisites(&state.db, concept_id).await?;
    Ok(Json(PrerequisiteListResponse {
        items: items.into_iter().map(|i| PrerequisiteItem { concept_id: i.concept_id, name: i.name }).collect(),
    }))
}

// POST /concepts/{id}/prerequisites
pub async fn post_prerequisite(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(concept_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<AddPrerequisiteRequest>,
) -> Result<(StatusCode, Json<PrerequisiteEdgeResponse>), AppError> {
    require_permission(&ctx, Resource::Module, Action::Create)?;
    concept::add_prerequisite(&state.db, concept_id, body.prerequisite_concept_id).await?;
    Ok((StatusCode::CREATED, Json(PrerequisiteEdgeResponse { concept_id, prerequisite_concept_id: body.prerequisite_concept_id })))
}

// DELETE /concepts/{id}/prerequisites/{prerequisite_concept_id}
pub async fn delete_prerequisite(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((concept_id, prerequisite_concept_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    require_permission(&ctx, Resource::Module, Action::Create)?;
    concept::remove_prerequisite(&state.db, concept_id, prerequisite_concept_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
