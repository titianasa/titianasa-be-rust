use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;

// Port of permissions.ts. Grows as new endpoints need authorization —
// one variant per resource this API exposes, not one per table (ADR-0006's
// "const map, not if-else per handler" instruction, ported as-is: a
// `match` here instead of a per-handler check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    OrganizationMembers,
    // Phase 31 rename: was Lesson — the section/item content tree within
    // one module (module_items), same 4 action rules as before.
    ModuleItem,
    QuestionBank,
    Attempt,
    Mastery,
    // Phase 31 rename: was Curriculum — covers both `modules` (the
    // self-nesting folder/module tree) and `programs` container-level
    // actions, since their authorization tier is identical today.
    Module,
    TutorProfile,
    LearningProduct,
    ProctoringPolicy,
    ProctoringSession,
    // Phase 32 (P32-002) — the new org-owned class/roster domain
    // (`classes`/`class_members`), deliberately separate from the
    // marketplace's `cohorts` (see phase-32 ticket for why).
    Class,
    // Phase 36 — "Periode" (batch/semester), pure org-admin territory:
    // a teacher creates classes, not academic terms.
    Period,
    // P40-001 (ADR-0014) — `/admin`, cross-organization by design
    // (unlike every other resource above, which is org-scoped or global
    // content). `is_admin_pusat_role` below is the ONE place role
    // eligibility is decided, so `pusat_kurikulum`/`pusat_ai`/
    // `pusat_bisnis`/`pusat_viewer` (ADR-0014 §1, not created yet) only
    // ever need to be added there, not hunted down across a match arm.
    AdminPusat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    View,
    ViewUnpublished,
    Create,
    Submit,
    SubmitReview,
    Publish,
    Review,
    // P40-001 — any admin-pusat write (approve/reject an AI proposal,
    // change model settings, roll back a version, ...). Every one of
    // these must go through `admin_audit::record` at the call site.
    Manage,
}

/// The one gate for every Admin Pusat permission check below. Today
/// only `platform_admin` — ADR-0014 §1's narrower `pusat_*` roles don't
/// exist in the DB yet, so adding one later is a one-line change here,
/// not a hunt through every `(AdminPusat, _)` match arm.
fn is_admin_pusat_role(role: Option<&str>) -> bool {
    matches!(role, Some("platform_admin"))
}

// The permission matrix itself, as a plain bool — split out from
// `require_permission` so a caller needing a *different* error on
// rejection can still reuse the same rule instead of duplicating the
// role list. Direct port of permissions.ts's isAllowed; keep the two
// files' role lists in sync when either changes.
pub fn is_allowed(role: Option<&str>, resource: Resource, action: Action) -> bool {
    let Some(role) = role else { return false };
    use Action::*;
    use Resource::*;

    match (resource, action) {
        (OrganizationMembers, View) => {
            matches!(role, "platform_admin" | "org_owner" | "academic_director")
        }
        (ModuleItem, ViewUnpublished) | (QuestionBank, ViewUnpublished) => matches!(
            role,
            "platform_admin" | "org_owner" | "academic_director" | "curriculum_developer" | "reviewer"
        ),
        (QuestionBank, Create)
        | (QuestionBank, SubmitReview)
        | (ModuleItem, SubmitReview)
        | (ModuleItem, Create)
        | (Module, Create) => {
            matches!(role, "platform_admin" | "org_owner" | "academic_director" | "curriculum_developer")
        }
        // ADR-0006 "Question bank: publish" / item publish — reviewer+
        // only, curriculum_developer excluded (can author, not approve
        // their own work).
        (QuestionBank, Publish) | (ModuleItem, Publish) => {
            matches!(role, "platform_admin" | "org_owner" | "academic_director" | "reviewer")
        }
        // ADR-0006's matrix only lists student for "Attempt: submit" —
        // taking an assessment is a learner action.
        (Attempt, Create) | (Attempt, Submit) => matches!(role, "platform_admin" | "student"),
        // ADR-0006 "Mastery/Progress: view" — everyone except
        // curriculum_developer/reviewer.
        (Mastery, View) => matches!(
            role,
            "platform_admin" | "org_owner" | "academic_director" | "teacher" | "tutor" | "student" | "parent"
        ),
        // P9-001 — assigning `tutor` in an org is an org-admin action,
        // same tier as organization_members:view.
        (TutorProfile, Create) => {
            matches!(role, "platform_admin" | "org_owner" | "academic_director")
        }
        // P9-003 — only a teacher/tutor can list their own products.
        // ADR-0006's addendum: `teacher`/`tutor` are one
        // permission-equivalent role, differentiated only by UI label
        // (organizations.type) — `tutor` stays listed as a legacy
        // match even though new promotions only ever write `teacher`.
        (LearningProduct, Create) => matches!(role, "platform_admin" | "teacher" | "tutor"),
        (ProctoringPolicy, Create) => {
            matches!(role, "platform_admin" | "org_owner" | "academic_director")
        }
        (ProctoringSession, Review) => {
            matches!(role, "platform_admin" | "org_owner" | "academic_director")
        }
        // Phase 36 — org-admin tier, same as TutorProfile::Create (P9-001).
        (Period, Create) | (Period, View) => {
            matches!(role, "platform_admin" | "org_owner" | "academic_director")
        }
        // P32-002 — a teacher/tutor creates/manages their own classes;
        // org-admin tier can too (oversight, and to hand a class to a
        // teacher). Narrower "is this caller's OWN class" ownership is
        // enforced separately in services/org_class.rs, same shape as
        // module_item.rs's can_edit_item — this only gates "can
        // create/manage a class AT ALL", not "this specific one".
        // `tutor` listed alongside `teacher` per ADR-0006's addendum
        // (see LearningProduct::Create above) — legacy match, new
        // promotions only ever write `teacher`.
        (Class, Create) | (Class, View) => {
            matches!(role, "platform_admin" | "org_owner" | "academic_director" | "teacher" | "tutor")
        }
        (AdminPusat, View) | (AdminPusat, Manage) => is_admin_pusat_role(Some(role)),
        _ => false,
    }
}

// Role-only check — for resources not scoped to one organization
// (curricula/question_banks have no organization_id column per ADR-0001).
pub fn require_permission(ctx: &AuthContext, resource: Resource, action: Action) -> Result<(), AppError> {
    if is_allowed(ctx.role.as_deref(), resource, action) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

// Same as require_permission, plus an org-membership check first — for
// org-scoped resources. platform_admin bypasses the scoping check
// entirely (ADR-0006: "Scoping selalu by organization_id kecuali
// platform_admin").
pub fn require_permission_in_org(
    ctx: &AuthContext,
    target_org: Uuid,
    resource: Resource,
    action: Action,
) -> Result<(), AppError> {
    let is_platform_admin = ctx.role.as_deref() == Some("platform_admin");
    if !is_platform_admin && ctx.organization_id != Some(target_org) {
        return Err(AppError::Forbidden);
    }
    require_permission(ctx, resource, action)
}
