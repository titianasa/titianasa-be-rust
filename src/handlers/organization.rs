use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::organization::{CreateOrganizationRequest, JoinOrganizationRequest};
use crate::models::requests::tutor::MembersQuery;
use crate::models::responses::organization::OrganizationResponse;
use crate::models::responses::tutor::MembersResponse;
use crate::services::organization;
use crate::state::AppState;

// GET /organizations/{id}/members
pub async fn get_members(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Query(query): Query<MembersQuery>,
) -> Result<Json<MembersResponse>, AppError> {
    let result = organization::get_members(&state.db, &ctx, id, query.cursor, query.limit).await?;
    Ok(Json(result))
}

// GET /organizations/{id}
pub async fn get_organization(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<OrganizationResponse>, AppError> {
    let result = organization::get_organization(&state.db, &ctx, id).await?;
    Ok(Json(result))
}

// POST /organizations — Phase 33 self-serve org creation.
pub async fn post_organization(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<CreateOrganizationRequest>,
) -> Result<(StatusCode, Json<OrganizationResponse>), AppError> {
    let result = organization::create_organization(&state.db, &ctx, body.name, body.r#type).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /organizations/join — Phase 33 self-serve join by invite code (slug).
pub async fn post_organization_join(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<JoinOrganizationRequest>,
) -> Result<Json<OrganizationResponse>, AppError> {
    let result = organization::join_organization_by_slug(&state.db, &ctx, &body.slug).await?;
    Ok(Json(result))
}

// POST /me/teacher-role — Phase 33 self-serve teacher assignment onto
// the caller's own current default org.
pub async fn post_me_teacher_role(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<StatusCode, AppError> {
    organization::self_assign_teacher(&state.db, &ctx).await?;
    Ok(StatusCode::NO_CONTENT)
}
