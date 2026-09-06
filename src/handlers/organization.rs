use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::tutor::MembersQuery;
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
