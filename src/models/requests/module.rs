use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct CreateModuleRequest {
    pub parent_id: Option<Uuid>,
    pub is_folder: bool,
    // Required unless is_folder — see modules_subject_required_unless_folder.
    pub subject_id: Option<Uuid>,
    pub code: Option<String>,
    pub title: String,
    pub description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateModuleRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReorderModulesRequest {
    pub parent_id: Option<Uuid>,
    pub ordered_ids: Vec<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AddModulePrerequisiteRequest {
    pub prerequisite_module_id: Uuid,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateModuleItemRequest {
    pub parent_id: Option<Uuid>,
    pub node_type: String,
    pub title: String,
    // item-only (node_type='item'); ignored for a section.
    pub content_type: Option<String>,
    pub content: Option<String>,
    pub format: Option<String>,
    pub concept_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateModuleItemMetaRequest {
    pub title: Option<String>,
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReorderModuleItemsRequest {
    pub module_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub ordered_ids: Vec<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateModuleItemContentRequest {
    pub content: String,
    pub format: String,
}
