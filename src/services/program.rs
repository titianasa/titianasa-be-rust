use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::program::{AttachModuleRequest, CreateProgramRequest, ReorderProgramModulesRequest, UpdateProgramRequest};
use crate::models::responses::program::{ListProgramsResponse, ProgramModuleNode, ProgramResponse, ProgramSummary, ProgramTree};
use crate::services::permissions::{require_permission, Action, Resource};

// Phase 31 (P31-002) — flat, student/enrollment-facing container.
// Replaces "curricula" 1:1 in role. Modules attach via program_modules
// (M2M — the same module can be reused across N programs), which
// curricula/levels/units structurally could never allow (a unit could
// only ever belong to one level belonging to one curriculum).

struct ProgramRow {
    id: Uuid,
    code: String,
    name: String,
    description: Option<String>,
    status: String,
}

fn to_summary(p: &ProgramRow) -> ProgramSummary {
    ProgramSummary { id: p.id, code: p.code.clone(), name: p.name.clone(), description: p.description.clone(), status: p.status.clone() }
}

async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ProgramRow>, AppError> {
    let row = sqlx::query_as!(ProgramRow, r#"select id, code, name, description, status from programs where id = $1"#, id).fetch_optional(pool).await?;
    Ok(row)
}

// POST /programs — always status = 'draft'.
pub async fn create(pool: &PgPool, ctx: &AuthContext, req: CreateProgramRequest) -> Result<ProgramResponse, AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    let row = sqlx::query!(
        r#"insert into programs (subject_id, code, name, description, framework, status) values ($1, $2, $3, $4, $5, 'draft')
           returning id, status"#,
        req.subject_id,
        req.code,
        req.name,
        req.description,
        req.framework,
    )
    .fetch_one(pool)
    .await?;
    Ok(ProgramResponse { id: row.id, status: row.status })
}

// GET /programs — Content Studio: list every program, no
// filter/pagination (an admin dataset, not a high-traffic list). Open
// read, same convention as the old curricula list.
pub async fn list(pool: &PgPool) -> Result<ListProgramsResponse, AppError> {
    let rows = sqlx::query_as!(ProgramRow, r#"select id, code, name, description, status from programs order by code asc"#).fetch_all(pool).await?;
    Ok(ListProgramsResponse { items: rows.iter().map(to_summary).collect() })
}

// GET /programs/{id}
pub async fn get(pool: &PgPool, id: Uuid) -> Result<ProgramSummary, AppError> {
    let row = find_by_id(pool, id).await?.ok_or(AppError::NotFound("program_not_found"))?;
    Ok(to_summary(&row))
}

// PATCH /programs/{id}
pub async fn update(pool: &PgPool, ctx: &AuthContext, id: Uuid, req: UpdateProgramRequest) -> Result<ProgramSummary, AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    let existing = find_by_id(pool, id).await?.ok_or(AppError::NotFound("program_not_found"))?;
    let name = req.name.unwrap_or(existing.name);
    let description = req.description.or(existing.description);
    let row = sqlx::query_as!(
        ProgramRow,
        r#"update programs set name = $2, description = $3, updated_at = now() where id = $1
           returning id, code, name, description, status"#,
        id,
        name,
        description,
    )
    .fetch_one(pool)
    .await?;
    Ok(to_summary(&row))
}

// POST /programs/{id}/modules — attach a module (idempotent on
// conflict, matching module_prerequisites/program_modules' PK-based
// upsert-nothing convention elsewhere in this codebase).
pub async fn attach_module(pool: &PgPool, ctx: &AuthContext, program_id: Uuid, req: AttachModuleRequest) -> Result<(), AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    find_by_id(pool, program_id).await?.ok_or(AppError::NotFound("program_not_found"))?;

    let next_order: i32 = sqlx::query_scalar!(r#"select coalesce(max(order_index), -1) + 1 as "next!" from program_modules where program_id = $1"#, program_id)
        .fetch_one(pool)
        .await?;

    sqlx::query!(
        r#"insert into program_modules (program_id, module_id, order_index, label_override) values ($1, $2, $3, $4)
           on conflict (program_id, module_id) do update set label_override = excluded.label_override"#,
        program_id,
        req.module_id,
        next_order,
        req.label_override,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// DELETE /programs/{id}/modules/{module_id}
pub async fn detach_module(pool: &PgPool, ctx: &AuthContext, program_id: Uuid, module_id: Uuid) -> Result<(), AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    sqlx::query!(r#"delete from program_modules where program_id = $1 and module_id = $2"#, program_id, module_id).execute(pool).await?;
    Ok(())
}

// POST /programs/{id}/modules/reorder
pub async fn reorder_modules(pool: &PgPool, ctx: &AuthContext, program_id: Uuid, req: ReorderProgramModulesRequest) -> Result<(), AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    let mut tx = pool.begin().await?;
    for (index, module_id) in req.ordered_module_ids.iter().enumerate() {
        sqlx::query!(
            r#"update program_modules set order_index = $3 where program_id = $1 and module_id = $2"#,
            program_id,
            module_id,
            index as i32,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

// GET /programs/{id}/tree — direct successor of the old
// GET /curricula/{id}/tree, one join hop deeper (program -> M2M ->
// modules), same fetch-flat/assemble-in-memory technique.
pub async fn get_tree(pool: &PgPool, program_id: Uuid) -> Result<ProgramTree, AppError> {
    let program = find_by_id(pool, program_id).await?.ok_or(AppError::NotFound("program_not_found"))?;

    let rows = sqlx::query!(
        r#"select pm.module_id, m.title, m.is_folder, pm.label_override, pm.order_index
           from program_modules pm inner join modules m on m.id = pm.module_id
           where pm.program_id = $1 order by pm.order_index asc"#,
        program_id,
    )
    .fetch_all(pool)
    .await?;

    let modules = rows
        .into_iter()
        .map(|r| ProgramModuleNode { module_id: r.module_id, title: r.title, is_folder: r.is_folder, label_override: r.label_override, order_index: r.order_index })
        .collect();

    Ok(ProgramTree { program: to_summary(&program), modules })
}
