use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct PresignedUploadRequest {
    pub content_type: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfirmUploadRequest {
    pub asset_id: Uuid,
    pub content_type: String,
    pub visibility: String,
    pub filename: Option<String>,
    pub folder_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ListAssetsQuery {
    pub folder_id: Option<Uuid>,
    pub r#type: Option<String>,
    pub cursor: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct RenameAssetRequest {
    pub filename: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct MoveAssetRequest {
    pub folder_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ShareRequest {
    pub principal_type: String,
    pub principal_id: String,
    pub permission: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
    pub parent_folder_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct RenameFolderRequest {
    pub name: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct MoveFolderRequest {
    pub parent_folder_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ListDriveQuery {
    pub folder_id: Option<Uuid>,
    pub cursor: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ListTrashQuery {
    pub cursor: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ShareCandidatesQuery {
    pub query: Option<String>,
    pub limit: Option<i64>,
}
