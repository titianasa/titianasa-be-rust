use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::org_class::CreateClassRequest;
use crate::models::responses::org_class::{ClassDetailResponse, ClassListResponse, ClassMemberResponse, ClassSummaryResponse};
use crate::services::permissions::{require_permission_in_org, Action, Resource};

// Phase 32 (P32-002) — a new, minimal, org-owned class (roster of
// students taught by one org `teacher`, optionally tied to a
// module/program). Deliberately separate from the marketplace's
// `cohorts`/`class_sessions` — see migrations/0029_classes.sql's header
// for why. No session-scheduling/attendance here yet; that's a fast-
// follow once this roster-level v1 ships.

// Phase 35 — pub(crate) so services/org_class_session.rs,
// org_attendance.rs, org_attendance_verification.rs, and attendance_qr.rs
// can reuse the exact same "who manages this class" gate instead of
// re-deriving it.
pub(crate) fn is_org_admin_tier(role: Option<&str>) -> bool {
    matches!(role, Some("org_owner") | Some("academic_director") | Some("platform_admin"))
}

pub(crate) struct ClassRow {
    pub(crate) organization_id: Uuid,
    pub(crate) teacher_id: Uuid,
}

pub(crate) async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ClassRow>, AppError> {
    let row = sqlx::query_as!(
        ClassRow,
        r#"select organization_id, teacher_id from classes where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// Shared gate for get_detail/add_member/remove_member (and, Phase 35,
// every session/attendance write): org-admin tier (of THAT class's org)
// always passes; a `teacher` caller additionally must be the class's own
// teacher — mirrors module_item.rs's can_edit_item "author tier OR
// narrower ownership" shape.
pub(crate) async fn assert_can_manage_class(pool: &PgPool, ctx: &AuthContext, class_id: Uuid) -> Result<ClassRow, AppError> {
    let class = find_by_id(pool, class_id).await?.ok_or(AppError::NotFound("class_not_found"))?;
    require_permission_in_org(ctx, class.organization_id, Resource::Class, Action::Create)?;
    if !is_org_admin_tier(ctx.role.as_deref()) && class.teacher_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }
    Ok(class)
}

// Phase 35 — the narrower "may I read/act on my OWN membership" gate:
// the manage-tier check above passes, OR the caller is a plain member
// (row in class_members) of this exact class. Used by the two
// student-initiated attendance paths (session-QR self-check-in,
// listing your own attendance) — a student should never need
// Resource::Class/Create permission just to check themselves in.
pub(crate) async fn assert_is_member_or_manager(pool: &PgPool, ctx: &AuthContext, class_id: Uuid) -> Result<ClassRow, AppError> {
    let class = find_by_id(pool, class_id).await?.ok_or(AppError::NotFound("class_not_found"))?;
    let is_manager = require_permission_in_org(ctx, class.organization_id, Resource::Class, Action::Create).is_ok()
        && (is_org_admin_tier(ctx.role.as_deref()) || class.teacher_id == ctx.user_id);
    if is_manager {
        return Ok(class);
    }
    let membership = sqlx::query_scalar!(
        r#"select 1 as "exists!" from class_members where class_id = $1 and student_id = $2"#,
        class_id,
        ctx.user_id,
    )
    .fetch_optional(pool)
    .await?;
    if membership.is_none() {
        return Err(AppError::Forbidden);
    }
    Ok(class)
}

async fn get_summary(pool: &PgPool, id: Uuid) -> Result<Option<ClassSummaryResponse>, AppError> {
    let row = sqlx::query_as!(
        ClassSummaryResponse,
        r#"select c.id, c.name, c.description, c.status, c.teacher_id, u.name as teacher_name,
                  c.module_id, m.title as "module_title?", c.program_id, p.name as "program_title?",
                  c.period_id, pd.name as "period_name?",
                  c.created_at,
                  (select count(*) from class_members cm where cm.class_id = c.id) as "member_count!"
           from classes c
           inner join users u on u.id = c.teacher_id
           left join modules m on m.id = c.module_id
           left join programs p on p.id = c.program_id
           left join periods pd on pd.id = c.period_id
           where c.id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// POST /classes
pub async fn create(pool: &PgPool, ctx: &AuthContext, req: CreateClassRequest) -> Result<ClassSummaryResponse, AppError> {
    require_permission_in_org(ctx, req.organization_id, Resource::Class, Action::Create)?;

    let teacher_id = match req.teacher_id {
        Some(t) if t != ctx.user_id && !is_org_admin_tier(ctx.role.as_deref()) => {
            return Err(AppError::Forbidden);
        }
        Some(t) => t,
        None => ctx.user_id,
    };

    if req.name.trim().is_empty() {
        return Err(AppError::UnprocessableEntity("name_required", "nama kelas wajib diisi".to_string()));
    }

    let id = sqlx::query_scalar!(
        r#"insert into classes (organization_id, teacher_id, module_id, program_id, period_id, name, description)
           values ($1, $2, $3, $4, $5, $6, $7)
           returning id"#,
        req.organization_id,
        teacher_id,
        req.module_id,
        req.program_id,
        req.period_id,
        req.name,
        req.description,
    )
    .fetch_one(pool)
    .await?;

    get_summary(pool, id).await?.ok_or(AppError::NotFound("class_not_found"))
}

// PATCH /classes/{id} — reassign which module/program/period this class
// links to, after creation (not just at create time). See
// UpdateClassLinksRequest's own doc comment for the full-replace
// semantics.
pub async fn update_links(pool: &PgPool, ctx: &AuthContext, class_id: Uuid, req: crate::models::requests::org_class::UpdateClassLinksRequest) -> Result<ClassSummaryResponse, AppError> {
    assert_can_manage_class(pool, ctx, class_id).await?;

    sqlx::query!(
        r#"update classes set module_id = $2, program_id = $3, period_id = $4, updated_at = now() where id = $1"#,
        class_id,
        req.module_id,
        req.program_id,
        req.period_id,
    )
    .execute(pool)
    .await?;

    get_summary(pool, class_id).await?.ok_or(AppError::NotFound("class_not_found"))
}

// GET /classes?organization_id= — a `teacher` caller sees only their
// own classes; org-admin tier sees every class in the org (oversight).
pub async fn list_for_caller(pool: &PgPool, ctx: &AuthContext, organization_id: Uuid) -> Result<ClassListResponse, AppError> {
    require_permission_in_org(ctx, organization_id, Resource::Class, Action::View)?;
    let admin_tier = is_org_admin_tier(ctx.role.as_deref());
    let scope_to_teacher = if admin_tier { None } else { Some(ctx.user_id) };

    let rows = sqlx::query_as!(
        ClassSummaryResponse,
        r#"select c.id, c.name, c.description, c.status, c.teacher_id, u.name as teacher_name,
                  c.module_id, m.title as "module_title?", c.program_id, p.name as "program_title?",
                  c.period_id, pd.name as "period_name?",
                  c.created_at,
                  (select count(*) from class_members cm where cm.class_id = c.id) as "member_count!"
           from classes c
           inner join users u on u.id = c.teacher_id
           left join modules m on m.id = c.module_id
           left join programs p on p.id = c.program_id
           left join periods pd on pd.id = c.period_id
           where c.organization_id = $1 and ($2::uuid is null or c.teacher_id = $2)
           order by c.created_at desc"#,
        organization_id,
        scope_to_teacher,
    )
    .fetch_all(pool)
    .await?;
    Ok(ClassListResponse { items: rows })
}

// GET /classes/{id}
pub async fn get_detail(pool: &PgPool, ctx: &AuthContext, class_id: Uuid) -> Result<ClassDetailResponse, AppError> {
    assert_can_manage_class(pool, ctx, class_id).await?;
    let summary = get_summary(pool, class_id).await?.ok_or(AppError::NotFound("class_not_found"))?;

    let members = sqlx::query_as!(
        ClassMemberResponse,
        r#"select u.id as "student_id!", u.name as "name!", u.email as "email!", cm.joined_at as "joined_at!"
           from class_members cm inner join users u on u.id = cm.student_id
           where cm.class_id = $1
           order by u.name asc"#,
        class_id,
    )
    .fetch_all(pool)
    .await?;

    Ok(ClassDetailResponse { summary, members })
}

// POST /classes/{id}/members
pub async fn add_member(pool: &PgPool, ctx: &AuthContext, class_id: Uuid, student_id: Uuid) -> Result<ClassMemberResponse, AppError> {
    assert_can_manage_class(pool, ctx, class_id).await?;

    sqlx::query!(
        r#"insert into class_members (class_id, student_id) values ($1, $2) on conflict do nothing"#,
        class_id,
        student_id,
    )
    .execute(pool)
    .await?;

    let member = sqlx::query_as!(
        ClassMemberResponse,
        r#"select u.id as "student_id!", u.name as "name!", u.email as "email!", cm.joined_at as "joined_at!"
           from class_members cm inner join users u on u.id = cm.student_id
           where cm.class_id = $1 and cm.student_id = $2"#,
        class_id,
        student_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("class_member_not_found"))?;

    Ok(member)
}

// DELETE /classes/{id}/members/{student_id}
pub async fn remove_member(pool: &PgPool, ctx: &AuthContext, class_id: Uuid, student_id: Uuid) -> Result<(), AppError> {
    assert_can_manage_class(pool, ctx, class_id).await?;
    sqlx::query!(r#"delete from class_members where class_id = $1 and student_id = $2"#, class_id, student_id)
        .execute(pool)
        .await?;
    Ok(())
}
