use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::responses::organization::OrganizationResponse;
use crate::models::responses::tutor::{MemberRowResponse, MembersResponse};
use crate::services::permissions::{require_permission_in_org, Action, Resource};

// Port of organization_repository.ts's listMembers — keyset-paginated by
// user_id (opaque cursor = last row's id), per api-contract.md's
// pagination rule. The optional cursor is expressed as a runtime NULL
// check rather than building dynamic SQL, so the compile-time `query!`
// macro still has one static query to validate.
async fn list_members_rows(
    pool: &PgPool,
    organization_id: Uuid,
    cursor: Option<Uuid>,
    limit: i64,
) -> Result<Vec<MemberRowResponse>, AppError> {
    let rows = sqlx::query!(
        r#"select u.id as "user_id!", u.name as "name!", r.role as "role!"
           from user_organization_roles r
           inner join users u on u.id = r.user_id
           where r.organization_id = $1
             and ($2::uuid is null or u.id > $2)
           order by u.id asc
           limit $3"#,
        organization_id,
        cursor,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| MemberRowResponse { user_id: r.user_id, name: r.name, role: r.role })
        .collect())
}

// Port of organization_handler.ts's getMembers. Auth: role
// org_owner/academic_director/platform_admin in that org.
pub async fn get_members(
    pool: &PgPool,
    ctx: &AuthContext,
    organization_id: Uuid,
    cursor: Option<Uuid>,
    limit: Option<i64>,
) -> Result<MembersResponse, AppError> {
    require_permission_in_org(ctx, organization_id, Resource::OrganizationMembers, Action::View)?;

    let limit = limit.unwrap_or(20).clamp(1, 100);
    let mut rows = list_members_rows(pool, organization_id, cursor, limit + 1).await?;

    let next_cursor = if rows.len() > limit as usize {
        rows.pop().map(|r| r.user_id)
    } else {
        None
    };

    Ok(MembersResponse { items: rows, next_cursor })
}

// Phase 33 — self-serve teacher assignment onto the caller's OWN
// current default org (roles[0]-equivalent — same "which org" every
// other page already resolves via find_all_roles). No permission gate
// beyond being authenticated: this is self-service onto your own
// membership, not an admin action over someone else (contrast
// tutor::assign_tutor, the org-admin "promote someone else" path).
pub async fn self_assign_teacher(pool: &PgPool, ctx: &AuthContext) -> Result<(), AppError> {
    let roles = crate::services::auth::find_all_roles(pool, ctx.user_id).await?;
    let organization_id = roles
        .first()
        .map(|r| r.organization_id)
        .ok_or(AppError::UnprocessableEntity("no_organization", "kamu belum tergabung di organisasi mana pun".to_string()))?;

    sqlx::query!(
        r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, 'teacher')
           on conflict (user_id, organization_id, role) do nothing"#,
        ctx.user_id,
        organization_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Slugifies a name for `organizations.slug` (unique, used as the
// human-shareable "invite code" — see join_organization_by_slug).
// Lowercase alnum with single dashes between runs of anything else;
// falls back to a fixed word if the name has no ASCII alnum at all
// (e.g. a name in a non-Latin script).
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    let slug: String = slug.chars().take(60).collect();
    if slug.is_empty() { "organisasi".to_string() } else { slug }
}

// Phase 33 — self-serve organization creation (no self-serve path
// existed before this; every org until now was either the one shared
// platform org or manually seeded). The creator becomes `org_owner`.
// `platform` is deliberately not an accepted `org_type` — reserved for
// the one auto-created shared org (see auth.rs's
// find_or_create_platform_org).
pub async fn create_organization(pool: &PgPool, ctx: &AuthContext, name: String, org_type: String) -> Result<OrganizationResponse, AppError> {
    if name.trim().is_empty() {
        return Err(AppError::UnprocessableEntity("name_required", "nama organisasi wajib diisi".to_string()));
    }
    if org_type != "school" && org_type != "tutor_org" {
        return Err(AppError::UnprocessableEntity("invalid_type", r#"type harus "school" atau "tutor_org""#.to_string()));
    }

    let base_slug = slugify(&name);
    let org = match sqlx::query_as!(
        OrganizationResponse,
        r#"insert into organizations (name, slug, type) values ($1, $2, $3)
           on conflict (slug) do nothing
           returning id, name, slug, type"#,
        name,
        base_slug,
        org_type,
    )
    .fetch_optional(pool)
    .await?
    {
        Some(org) => org,
        // Slug collision (another org already has this exact slugified
        // name) — retry once with a short random suffix, same "just
        // retry with a suffix" idea find_or_create_platform_org uses
        // for its own conflict.
        None => {
            let suffixed_slug = format!("{base_slug}-{}", &Uuid::new_v4().simple().to_string()[..4]);
            sqlx::query_as!(
                OrganizationResponse,
                r#"insert into organizations (name, slug, type) values ($1, $2, $3)
                   returning id, name, slug, type"#,
                name,
                suffixed_slug,
                org_type,
            )
            .fetch_one(pool)
            .await?
        }
    };

    sqlx::query!(
        r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, 'org_owner')
           on conflict (user_id, organization_id, role) do nothing"#,
        ctx.user_id,
        org.id,
    )
    .execute(pool)
    .await?;

    Ok(org)
}

// Phase 33 — self-serve join via invite code (the org's own slug).
// Deliberately not a public directory search (see this phase's ticket
// for why) — you need the exact slug, shared out-of-band by an admin.
pub async fn join_organization_by_slug(pool: &PgPool, ctx: &AuthContext, slug: &str) -> Result<OrganizationResponse, AppError> {
    let org = sqlx::query_as!(OrganizationResponse, r#"select id, name, slug, type from organizations where slug = $1"#, slug)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("organization_not_found"))?;

    sqlx::query!(
        r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, 'student')
           on conflict (user_id, organization_id, role) do nothing"#,
        ctx.user_id,
        org.id,
    )
    .execute(pool)
    .await?;

    Ok(org)
}

// GET /organizations/{id} — same sensitivity tier as get_members (the
// slug doubles as the org's invite code, so this stays admin-gated
// rather than open to any member).
pub async fn get_organization(pool: &PgPool, ctx: &AuthContext, organization_id: Uuid) -> Result<OrganizationResponse, AppError> {
    require_permission_in_org(ctx, organization_id, Resource::OrganizationMembers, Action::View)?;
    sqlx::query_as!(OrganizationResponse, r#"select id, name, slug, type from organizations where id = $1"#, organization_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("organization_not_found"))
}

#[derive(Debug, serde::Serialize)]
pub struct ShareCandidate {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
}

// Port of organization_repository.ts's searchShareCandidates — who a
// Drive (and, Phase 31 P31-008, module_item) share dialog can offer as
// a target, scoped to the caller's own organization. Deliberately NOT
// get_members (role-gated too tightly for a share-picker) — no
// permission check at all here, only implicit scoping to
// organization_id (drive.rs's caller passes ctx's own active org, never
// an arbitrary one). Matches on email too (not just name, the Drive-only
// original scope) since P31-008's share dialog is explicitly a
// type-an-email-and-see-suggestions flow.
// P32 fix — a member can hold more than one role in the same org
// (ADR-0006), so the old plain join returned that person once per role
// row (e.g. a platform_admin who's also a tutor showed up twice,
// breaking the FE's `key={user_id}` list — real bug, surfaced by
// exactly that kind of multi-role account). `distinct` collapses back
// to one row per person regardless of how many roles they hold here.
pub async fn search_share_candidates(pool: &PgPool, organization_id: Uuid, query: Option<&str>, limit: i64) -> Result<Vec<ShareCandidate>, AppError> {
    let pattern = query.map(|q| format!("%{q}%"));
    let rows = sqlx::query_as!(
        ShareCandidate,
        r#"select distinct u.id as "user_id!", u.name as "name!", u.email as "email!"
           from user_organization_roles r
           inner join users u on u.id = r.user_id
           where r.organization_id = $1
             and ($2::text is null or u.name ilike $2 or u.email ilike $2)
           order by u.name asc
           limit $3"#,
        organization_id,
        pattern,
        limit,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
