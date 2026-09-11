use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct OrganizationResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub r#type: String,
}
