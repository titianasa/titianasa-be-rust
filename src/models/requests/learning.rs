#[derive(Debug, serde::Deserialize)]
pub struct QueueQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AddPrerequisiteRequest {
    pub prerequisite_concept_id: uuid::Uuid,
}
