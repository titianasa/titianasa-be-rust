use uuid::Uuid;

// A "folder" and a "module" are the same row (is_folder distinguishes
// them) — one response shape covers both, matching ParaLabs' own
// org_modules table.
#[derive(Debug, serde::Serialize)]
pub struct ModuleResponse {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub is_folder: bool,
    pub subject_id: Option<Uuid>,
    pub code: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub version: i32,
    pub order_index: i32,
    pub generated_by: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ListModulesResponse {
    pub items: Vec<ModuleResponse>,
}

#[derive(Debug, serde::Serialize)]
pub struct ModuleAncestor {
    pub id: Uuid,
    pub title: String,
    pub is_folder: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct ModuleAncestorsResponse {
    pub items: Vec<ModuleAncestor>,
}

#[derive(Debug, serde::Serialize)]
pub struct ModulePrerequisite {
    pub module_id: Uuid,
    pub title: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ModulePrerequisitesResponse {
    pub items: Vec<ModulePrerequisite>,
}

// A module's item TREE — the second, independent tree (section/item,
// depth<=5), nested in one response since that depth cap keeps it small.
#[derive(Debug, serde::Serialize)]
pub struct ModuleItemNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub node_type: String,
    pub title: String,
    pub order_index: i32,
    pub content_type: Option<String>,
    pub status: String,
    pub generated_by: String,
    pub children: Vec<ModuleItemNode>,
}

#[derive(Debug, serde::Serialize)]
pub struct ModuleItemTreeResponse {
    pub items: Vec<ModuleItemNode>,
}

#[derive(Debug, serde::Serialize)]
pub struct ModuleItemBlockResponse {
    pub id: Uuid,
    pub r#type: String,
    pub order_index: i32,
    pub data: serde_json::Value,
    pub raw_source: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ModuleItemDetailResponse {
    pub id: Uuid,
    pub module_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub node_type: String,
    pub title: String,
    pub content_type: Option<String>,
    pub status: String,
    pub blocks: Vec<ModuleItemBlockResponse>,
    pub qa_report: Option<serde_json::Value>,
    pub generated_by: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ModuleItemStatusResponse {
    pub id: Uuid,
    pub status: String,
    pub qa_report: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
pub struct SpeakingPromptResponse {
    pub text: String,
}

