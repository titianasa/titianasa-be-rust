use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct PrerequisiteItem {
    pub concept_id: Uuid,
    pub name: String,
}

#[derive(Debug, serde::Serialize)]
pub struct PrerequisiteListResponse {
    pub items: Vec<PrerequisiteItem>,
}

#[derive(Debug, serde::Serialize)]
pub struct PrerequisiteEdgeResponse {
    pub concept_id: Uuid,
    pub prerequisite_concept_id: Uuid,
}
