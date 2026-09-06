use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

// Assets (P1-010/P2-010) + Drive (folders/sharing/activity/trash). All
// protected — every route requires a bearer token, no public routes
// here.
pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/assets", post(handlers::asset::post_upload).get(handlers::asset::get_assets))
        .route("/assets/upload", post(handlers::asset::post_upload))
        .route("/assets/presigned-upload", post(handlers::asset::post_presigned_upload))
        .route("/assets/confirm", post(handlers::asset::post_confirm))
        .route("/assets/{id}", get(handlers::asset::get_asset).delete(handlers::asset::delete_asset))
        .route("/assets/{id}/rename", post(handlers::asset::post_rename_asset))
        .route("/assets/{id}/move", post(handlers::asset::post_move_asset))
        .route("/assets/{id}/restore", post(handlers::asset::post_restore_asset))
        .route("/assets/{id}/permanent", axum::routing::delete(handlers::asset::delete_asset_permanent))
        .route("/assets/{id}/shares", get(handlers::asset::get_asset_shares).post(handlers::asset::post_asset_share))
        .route("/assets/{id}/shares/{share_id}", axum::routing::delete(handlers::asset::delete_asset_share))
        .route("/assets/{id}/activity", get(handlers::asset::get_asset_activity))
        .route("/folders", post(handlers::folder::post_folder))
        .route("/folders/{id}", axum::routing::delete(handlers::folder::delete_folder))
        .route("/folders/{id}/rename", post(handlers::folder::post_rename_folder))
        .route("/folders/{id}/move", post(handlers::folder::post_move_folder))
        .route("/folders/{id}/restore", post(handlers::folder::post_restore_folder))
        .route("/folders/{id}/permanent", axum::routing::delete(handlers::folder::delete_folder_permanent))
        .route("/folders/{id}/shares", get(handlers::folder::get_folder_shares).post(handlers::folder::post_folder_share))
        .route("/folders/{id}/shares/{share_id}", axum::routing::delete(handlers::folder::delete_folder_share))
        .route("/folders/{id}/activity", get(handlers::folder::get_folder_activity))
        .route("/drive", get(handlers::drive::get_drive))
        .route("/drive/trash", get(handlers::drive::get_drive_trash))
        .route("/drive/shared-with-me", get(handlers::drive::get_drive_shared_with_me))
        .route("/drive/share-candidates", get(handlers::drive::get_share_candidates))
}
