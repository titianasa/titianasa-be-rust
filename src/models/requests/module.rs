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
    // Migration 0039 — open-ended curriculum metadata (learning
    // objectives, difficulty, duration, source refs, an agent's
    // generation parameters). Deliberately unvalidated jsonb so a
    // curriculum agent can add NEW fields without a migration.
    pub metadata: Option<serde_json::Value>,
    // Migration 0042 — set to make this a REFERENCE row: it owns no
    // content, and every content read resolves to the library module
    // named here. This is how a learning path reuses the master
    // library instead of duplicating it.
    pub source_module_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateModuleRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub parent_id: Option<Uuid>,
    // Full replace when present, untouched when omitted — same
    // "form submits complete state" convention as UpdateQuizConfigRequest.
    pub metadata: Option<serde_json::Value>,
    // Ignored for a folder (folders have no subject). Added so a module
    // created before Content Studio had a real subject picker — every
    // one of them defaulted to a single hardcoded subject — can be
    // recategorized without recreating it from scratch.
    pub subject_id: Option<Uuid>,
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
    // Phase 37 — only meaningful when content_type="quiz"; the AI
    // quiz-generation path sets this at creation time, the Quiz
    // Builder sets it afterward via PATCH (see UpdateQuizConfigRequest).
    pub quiz_config: Option<serde_json::Value>,
    // Migration 0043 — set to author this ONE item under a different
    // subject than its module's, decided up front instead of creating
    // it then immediately reopening PATCH /module-items/{id}/subject.
    // None (the common case) leaves it inheriting the module's subject.
    pub subject_id: Option<Uuid>,
}

// Phase 37 — PATCH /module-items/{id}/quiz-config, the Quiz Builder's
// save action. Always a full replace of the { sections, question_groups }
// shape, same "form always submits complete state" reasoning as
// org_class.rs's UpdateClassLinksRequest.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateQuizConfigRequest {
    pub quiz_config: serde_json::Value,
}

// PATCH /module-items/{id}/lesson-plan — the Modul Belajar editor's
// save. Always the complete plan, same reasoning as UpdateQuizConfigRequest.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateLessonPlanRequest {
    pub lesson_plan: serde_json::Value,
    /// The plan being saved came straight out of `/ai/generate-lesson-plan`.
    /// Generation does not persist (the author reviews first), so this
    /// save is the only moment provenance can be recorded — without it
    /// every AI-written Modul Belajar is filed as hand-written, and the
    /// ✨ marker in the item tree lies about who wrote the material.
    #[serde(default)]
    pub ai_generated: bool,
}

// PATCH /module-items/{id}/guards — both always sent; null clears.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateItemGuardsRequest {
    pub guard_config: Option<serde_json::Value>,
    pub attendance_guard: Option<serde_json::Value>,
}

// PUT .../proctor-config — the level's own overrides; null clears them.
#[derive(Debug, serde::Deserialize)]
pub struct ProctorConfigRequest {
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
pub struct SetApprovalRequest {
    pub approved: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateModuleItemMetaRequest {
    pub title: Option<String>,
    pub parent_id: Option<Uuid>,
}

// PATCH /module-items/{id}/subject — always applied verbatim: Some(id)
// sets/changes the override, None clears it back to "inherit from the
// module". See module_item::update_subject for why this is a dedicated
// endpoint instead of a field on UpdateModuleItemMetaRequest.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateModuleItemSubjectRequest {
    pub subject_id: Option<Uuid>,
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
