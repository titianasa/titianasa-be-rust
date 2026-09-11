use uuid::Uuid;

// Phase 32 (P32-002) — requests for the new org-owned class/roster
// domain. Named `org_class` (not `class`) to keep it unambiguous next
// to the pre-existing, unrelated `class_session`/`cohort` (marketplace)
// modules — same word, deliberately different feature (see
// migrations/0029_classes.sql's header for why they're not the same
// table).

#[derive(Debug, serde::Deserialize)]
pub struct CreateClassRequest {
    pub organization_id: Uuid,
    /// Only an org-admin-tier caller may set this to someone other than
    /// themselves — a `teacher` caller always creates for themselves.
    pub teacher_id: Option<Uuid>,
    pub module_id: Option<Uuid>,
    pub program_id: Option<Uuid>,
    pub period_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AddClassMemberRequest {
    pub student_id: Uuid,
}

// Phase 36 — PATCH /classes/{id}. Full-replace semantics for these
// three link fields (always send the complete current picker state,
// never a sparse patch) — a field that's `null` genuinely clears that
// link, there's no separate "omit to leave unchanged" case to support
// since the edit form always submits all three together.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateClassLinksRequest {
    pub module_id: Option<Uuid>,
    pub program_id: Option<Uuid>,
    pub period_id: Option<Uuid>,
}
