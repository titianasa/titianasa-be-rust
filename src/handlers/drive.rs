use axum::{
    extract::{Query, State},
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::handlers::asset::{to_detail, AssetDetailResponse};
use crate::models::auth::AuthContext;
use crate::models::requests::drive::{ListDriveQuery, ListTrashQuery, ShareCandidatesQuery};
use crate::services::drive::{self, DriveListing};
use crate::services::folder::Folder;
use crate::services::organization::ShareCandidate;
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct DriveListingResponse {
    pub folders: Vec<Folder>,
    pub assets: Vec<AssetDetailResponse>,
    pub breadcrumb: Vec<Folder>,
    pub next_cursor: Option<Uuid>,
}

async fn to_response(state: &AppState, listing: DriveListing) -> Result<DriveListingResponse, AppError> {
    let mut assets = Vec::with_capacity(listing.assets.len());
    for a in listing.assets {
        assets.push(to_detail(state.storage.as_ref(), &state.config, a).await?);
    }
    Ok(DriveListingResponse { folders: listing.folders, assets, breadcrumb: listing.breadcrumb, next_cursor: listing.next_cursor })
}

// GET /drive?folder_id=&cursor=&limit=
pub async fn get_drive(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<ListDriveQuery>) -> Result<Json<DriveListingResponse>, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let listing = drive::list_drive(&state.db, &ctx, query.folder_id, query.cursor, limit).await?;
    Ok(Json(to_response(&state, listing).await?))
}

// GET /drive/trash?cursor=&limit=
pub async fn get_drive_trash(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<ListTrashQuery>) -> Result<Json<DriveListingResponse>, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let listing = drive::list_trash(&state.db, &ctx, query.cursor, limit).await?;
    Ok(Json(to_response(&state, listing).await?))
}

// GET /drive/shared-with-me
pub async fn get_drive_shared_with_me(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<DriveListingResponse>, AppError> {
    let listing = drive::list_shared_with_me(&state.db, &ctx).await?;
    Ok(Json(to_response(&state, listing).await?))
}

// GET /drive/share-candidates?query=&limit=
pub async fn get_share_candidates(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<ShareCandidatesQuery>) -> Result<Json<Vec<ShareCandidate>>, AppError> {
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    Ok(Json(drive::share_candidates(&state.db, &ctx, query.query.as_deref(), limit).await?))
}
