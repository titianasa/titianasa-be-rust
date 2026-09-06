use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
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

#[derive(Debug, serde::Serialize)]
pub struct ShareCandidate {
    pub user_id: Uuid,
    pub name: String,
}

// Port of organization_repository.ts's searchShareCandidates — who a
// Drive share dialog can offer as a target, scoped to the caller's own
// organization. Deliberately NOT get_members (role-gated too tightly for
// a share-picker) — no permission check at all here, only implicit
// scoping to organization_id (drive.rs's caller passes ctx's own active
// org, never an arbitrary one).
pub async fn search_share_candidates(pool: &PgPool, organization_id: Uuid, query: Option<&str>, limit: i64) -> Result<Vec<ShareCandidate>, AppError> {
    let pattern = query.map(|q| format!("%{q}%"));
    let rows = sqlx::query_as!(
        ShareCandidate,
        r#"select u.id as "user_id!", u.name as "name!"
           from user_organization_roles r
           inner join users u on u.id = r.user_id
           where r.organization_id = $1
             and ($2::text is null or u.name ilike $2)
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
