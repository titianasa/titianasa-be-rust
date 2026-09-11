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
    pub metadata: Option<serde_json::Value>,
    /// Set when this row is a reference into the master library — its
    /// content comes from that module, not from this one.
    pub source_module_id: Option<Uuid>,
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
    /// Learners only — why this item cannot be opened yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_reason: Option<String>,
    /// Learners only — whether they have finished it.
    pub completed: bool,
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
    // Phase 37 — set only when content_type="quiz"; { sections, question_groups }.
    pub quiz_config: Option<serde_json::Value>,
    pub status: String,
    pub blocks: Vec<ModuleItemBlockResponse>,
    pub qa_report: Option<serde_json::Value>,
    pub generated_by: String,
    /// Migration 0043 — set only when this item's subject diverges from
    /// its module's; None means "inherit the module's subject".
    pub subject_id: Option<Uuid>,
    /// Migration 0044 — set when an article is a sectioned Modul Belajar.
    pub lesson_plan: Option<serde_json::Value>,
    /// Migration 0045 — "Aturan Akses & Guard" and "Attendance Guard".
    pub guard_config: Option<serde_json::Value>,
    pub attendance_guard: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
pub struct LessonPlanSaveResponse {
    pub id: Uuid,
    pub status: String,
    pub qa_report: Option<serde_json::Value>,
    /// The plan as stored — ids filled in, text trimmed.
    pub lesson_plan: serde_json::Value,
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

