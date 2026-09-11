// Phase 33 — self-serve organization creation. `r#type` mirrors the
// `organizations.type` CHECK constraint minus `platform` (that value is
// reserved for the one auto-created shared org, never user-chosen).
#[derive(Debug, serde::Deserialize)]
pub struct CreateOrganizationRequest {
    pub name: String,
    pub r#type: String,
}

// Phase 33 — self-serve join via invite code (the org's own slug,
// shown to admin-tier members on the Organisasi page).
#[derive(Debug, serde::Deserialize)]
pub struct JoinOrganizationRequest {
    pub slug: String,
}
