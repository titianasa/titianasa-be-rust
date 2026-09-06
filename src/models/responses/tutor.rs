use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct MemberRowResponse {
    pub user_id: Uuid,
    pub name: String,
    pub role: String,
}

#[derive(Debug, serde::Serialize)]
pub struct MembersResponse {
    pub items: Vec<MemberRowResponse>,
    pub next_cursor: Option<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct TutorProfileResponse {
    pub user_id: Uuid,
    pub organization_id: Uuid,
    pub bio: String,
    pub specializations: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
pub struct TutorListRowResponse {
    pub user_id: Uuid,
    pub name: String,
    pub bio: String,
    pub specializations: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
pub struct TutorListResponse {
    pub items: Vec<TutorListRowResponse>,
}
