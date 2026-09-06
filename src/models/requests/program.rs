use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct CreateProgramRequest {
    pub subject_id: Uuid,
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub framework: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateProgramRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AttachModuleRequest {
    pub module_id: Uuid,
    pub label_override: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReorderProgramModulesRequest {
    pub ordered_module_ids: Vec<Uuid>,
}
