#[derive(Debug, serde::Deserialize)]
pub struct AssignTutorRequest {
    pub user_id: uuid::Uuid,
    pub bio: Option<String>,
    pub specializations: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateTutorProfileRequest {
    pub bio: Option<String>,
    pub specializations: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MembersQuery {
    pub cursor: Option<uuid::Uuid>,
    pub limit: Option<i64>,
}
