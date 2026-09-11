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
use crate::models::requests::org_class::{AddClassMemberRequest, CreateClassRequest, UpdateClassLinksRequest};
use crate::models::responses::org_class::{ClassDetailResponse, ClassListResponse, ClassMemberResponse, ClassSummaryResponse};
use crate::services::org_class;
use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct OrganizationIdQuery {
    pub organization_id: Uuid,
}

// GET /classes?organization_id=
pub async fn get_classes(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Query(query): Query<OrganizationIdQuery>,
) -> Result<Json<ClassListResponse>, AppError> {
    Ok(Json(org_class::list_for_caller(&state.db, &ctx, query.organization_id).await?))
}

// POST /classes
pub async fn post_class(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<CreateClassRequest>,
) -> Result<(StatusCode, Json<ClassSummaryResponse>), AppError> {
    let result = org_class::create(&state.db, &ctx, body).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /classes/{id}
pub async fn get_class(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<ClassDetailResponse>, AppError> {
    Ok(Json(org_class::get_detail(&state.db, &ctx, id).await?))
}

// PATCH /classes/{id} — reassign module/program/period links.
pub async fn patch_class(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<UpdateClassLinksRequest>,
) -> Result<Json<ClassSummaryResponse>, AppError> {
    Ok(Json(org_class::update_links(&state.db, &ctx, id, body).await?))
}

// POST /classes/{id}/members
pub async fn post_class_member(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<AddClassMemberRequest>,
) -> Result<(StatusCode, Json<ClassMemberResponse>), AppError> {
    let result = org_class::add_member(&state.db, &ctx, id, body.student_id).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// DELETE /classes/{id}/members/{student_id}
pub async fn delete_class_member(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, student_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    org_class::remove_member(&state.db, &ctx, id, student_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
