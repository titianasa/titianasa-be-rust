use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::module::{AddModulePrerequisiteRequest, CreateModuleRequest, ReorderModulesRequest, UpdateModuleRequest};
use crate::models::responses::module::{ListModulesResponse, ModuleAncestor, ModuleAncestorsResponse, ModulePrerequisite, ModulePrerequisitesResponse, ModuleResponse};
use crate::services::permissions::{require_permission, Action, Resource};

// Phase 31 (P31-002) — ParaLabs-style self-nesting module tree. A
// "folder" and a "module" are the SAME row (org_modules there, modules
// here), distinguished only by is_folder — modules can nest inside
// folders or inside other modules via the same parent_id column. This
// replaces "levels"+"units" as a concept. The item-tree WITHIN one
// module (module_items) is a second, independent tree — see
// services/module_item.rs.

struct ModuleRow {
    id: Uuid,
    parent_id: Option<Uuid>,
    is_folder: bool,
    subject_id: Option<Uuid>,
    code: Option<String>,
    title: String,
    description: Option<String>,
    status: String,
    version: i32,
    order_index: i32,
    generated_by: String,
}

fn to_response(m: &ModuleRow) -> ModuleResponse {
    ModuleResponse {
        id: m.id,
        parent_id: m.parent_id,
        is_folder: m.is_folder,
        subject_id: m.subject_id,
        code: m.code.clone(),
        title: m.title.clone(),
        description: m.description.clone(),
        status: m.status.clone(),
        version: m.version,
        order_index: m.order_index,
        generated_by: m.generated_by.clone(),
    }
}

async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ModuleRow>, AppError> {
    let row = sqlx::query_as!(
        ModuleRow,
        r#"select id, parent_id, is_folder, subject_id, code, title, description, status, version, order_index, generated_by
           from modules where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// Every ancestor of `id`, root-first, excluding `id` itself — mirrors
// folder.rs's find_ancestor_ids exactly (same adjacency-list tree
// pattern, different table).
async fn find_ancestor_ids(pool: &PgPool, id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"with recursive ancestors as (
             select m.id, m.parent_id, 0 as depth from modules m where m.id = $1
             union all
             select m.id, m.parent_id, a.depth + 1 from modules m join ancestors a on m.id = a.parent_id
           )
           select id as "id!" from ancestors where id != $1 order by depth desc"#,
        id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// Every descendant module of `id` (any depth), excluding `id` itself.
async fn find_descendant_ids(pool: &PgPool, id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"with recursive descendants as (
             select m.id from modules m where m.parent_id = $1
             union all
             select m.id from modules m join descendants d on m.parent_id = d.id
           )
           select id as "id!" from descendants"#,
        id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// POST /modules
pub async fn create(pool: &PgPool, ctx: &AuthContext, req: CreateModuleRequest) -> Result<ModuleResponse, AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;

    if !req.is_folder && req.subject_id.is_none() {
        return Err(AppError::UnprocessableEntity("subject_id_required", "subject_id is required for a module (not required for a folder)".to_string()));
    }
    if let Some(parent_id) = req.parent_id {
        find_by_id(pool, parent_id).await?.ok_or(AppError::NotFound("module_not_found"))?;
    }

    let row = sqlx::query_as!(
        ModuleRow,
        r#"insert into modules (parent_id, is_folder, subject_id, code, title, description, generated_by)
           values ($1, $2, $3, $4, $5, $6, 'human')
           returning id, parent_id, is_folder, subject_id, code, title, description, status, version, order_index, generated_by"#,
        req.parent_id,
        req.is_folder,
        req.subject_id,
        req.code,
        req.title,
        req.description,
    )
    .fetch_one(pool)
    .await?;
    Ok(to_response(&row))
}

// GET /modules/{id} — open read, same "no dedicated View row in the
// matrix" convention as the old Curriculum resource.
pub async fn get(pool: &PgPool, id: Uuid) -> Result<ModuleResponse, AppError> {
    let row = find_by_id(pool, id).await?.ok_or(AppError::NotFound("module_not_found"))?;
    Ok(to_response(&row))
}

// GET /modules?parent_id= — direct children (folders+modules), parent_id
// omitted lists root-level nodes.
pub async fn list_children(pool: &PgPool, parent_id: Option<Uuid>) -> Result<ListModulesResponse, AppError> {
    let rows = sqlx::query_as!(
        ModuleRow,
        r#"select id, parent_id, is_folder, subject_id, code, title, description, status, version, order_index, generated_by
           from modules where parent_id is not distinct from $1 order by order_index asc"#,
        parent_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(ListModulesResponse { items: rows.iter().map(to_response).collect() })
}

// GET /modules/{id}/ancestors — breadcrumb, root-first.
pub async fn list_ancestors(pool: &PgPool, id: Uuid) -> Result<ModuleAncestorsResponse, AppError> {
    find_by_id(pool, id).await?.ok_or(AppError::NotFound("module_not_found"))?;
    let ids = find_ancestor_ids(pool, id).await?;
    if ids.is_empty() {
        return Ok(ModuleAncestorsResponse { items: vec![] });
    }
    let rows = sqlx::query!(r#"select id, title, is_folder from modules where id = any($1)"#, &ids).fetch_all(pool).await?;
    let by_id: std::collections::HashMap<Uuid, (String, bool)> = rows.into_iter().map(|r| (r.id, (r.title, r.is_folder))).collect();
    let items = ids
        .into_iter()
        .filter_map(|id| by_id.get(&id).map(|(title, is_folder)| ModuleAncestor { id, title: title.clone(), is_folder: *is_folder }))
        .collect();
    Ok(ModuleAncestorsResponse { items })
}

// PATCH /modules/{id} — rename/edit/move. Moving into your own
// descendant is rejected the same way folder.rs's move_folder rejects it.
pub async fn update(pool: &PgPool, ctx: &AuthContext, id: Uuid, req: UpdateModuleRequest) -> Result<ModuleResponse, AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    let existing = find_by_id(pool, id).await?.ok_or(AppError::NotFound("module_not_found"))?;

    let new_parent_id = if let Some(parent_id) = req.parent_id {
        if parent_id == id {
            return Err(AppError::UnprocessableEntity("invalid_move", "a module cannot be moved into itself".to_string()));
        }
        find_by_id(pool, parent_id).await?.ok_or(AppError::NotFound("module_not_found"))?;
        let descendants = find_descendant_ids(pool, id).await?;
        if descendants.contains(&parent_id) {
            return Err(AppError::UnprocessableEntity("invalid_move", "a module cannot be moved into its own descendant".to_string()));
        }
        Some(parent_id)
    } else {
        existing.parent_id
    };

    let title = req.title.unwrap_or(existing.title);
    let description = req.description.or(existing.description);

    let row = sqlx::query_as!(
        ModuleRow,
        r#"update modules set title = $2, description = $3, parent_id = $4, updated_at = now() where id = $1
           returning id, parent_id, is_folder, subject_id, code, title, description, status, version, order_index, generated_by"#,
        id,
        title,
        description,
        new_parent_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(to_response(&row))
}

// POST /modules/reorder — batch sibling order_index rewrite, one
// transaction. No precedent elsewhere in this codebase for this exact
// pattern (existing order_index columns are only ever set once at
// creation).
pub async fn reorder(pool: &PgPool, ctx: &AuthContext, req: ReorderModulesRequest) -> Result<(), AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    let mut tx = pool.begin().await?;
    for (index, id) in req.ordered_ids.iter().enumerate() {
        sqlx::query!(
            r#"update modules set order_index = $2, updated_at = now() where id = $1 and parent_id is not distinct from $3"#,
            id,
            index as i32,
            req.parent_id,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

// --- Prerequisites — module graph, portable across every program that
// reuses the module. Mirrors concept.rs's add_prerequisite/
// remove_prerequisite/list_prerequisites cycle-check pattern exactly. ---

async fn find_transitive_prerequisite_ids(pool: &PgPool, id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"WITH RECURSIVE prereqs AS (
             SELECT mp.prerequisite_module_id AS id FROM module_prerequisites mp WHERE mp.module_id = $1
             UNION ALL
             SELECT mp.prerequisite_module_id AS id FROM module_prerequisites mp
             JOIN prereqs p ON mp.module_id = p.id
           )
           SELECT DISTINCT id as "id!" FROM prereqs"#,
        id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// GET /modules/{id}/prerequisites
pub async fn list_prerequisites(pool: &PgPool, module_id: Uuid) -> Result<ModulePrerequisitesResponse, AppError> {
    find_by_id(pool, module_id).await?.ok_or(AppError::NotFound("module_not_found"))?;
    let rows = sqlx::query!(
        r#"select m.id as module_id, m.title from module_prerequisites mp
           inner join modules m on m.id = mp.prerequisite_module_id
           where mp.module_id = $1"#,
        module_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(ModulePrerequisitesResponse { items: rows.into_iter().map(|r| ModulePrerequisite { module_id: r.module_id, title: r.title }).collect() })
}

// POST /modules/{id}/prerequisites
pub async fn add_prerequisite(pool: &PgPool, ctx: &AuthContext, module_id: Uuid, req: AddModulePrerequisiteRequest) -> Result<(), AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    if req.prerequisite_module_id == module_id {
        return Err(AppError::UnprocessableEntity("module_prerequisite_cycle", "a module cannot be its own prerequisite".to_string()));
    }
    find_by_id(pool, module_id).await?.ok_or(AppError::NotFound("module_not_found"))?;
    find_by_id(pool, req.prerequisite_module_id).await?.ok_or(AppError::NotFound("module_not_found"))?;

    let transitive = find_transitive_prerequisite_ids(pool, req.prerequisite_module_id).await?;
    if transitive.contains(&module_id) {
        return Err(AppError::UnprocessableEntity(
            "module_prerequisite_cycle",
            "the proposed prerequisite already depends on this module, directly or transitively".to_string(),
        ));
    }

    sqlx::query!(
        r#"insert into module_prerequisites (module_id, prerequisite_module_id) values ($1, $2) on conflict do nothing"#,
        module_id,
        req.prerequisite_module_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// DELETE /modules/{id}/prerequisites/{prerequisite_module_id}
pub async fn remove_prerequisite(pool: &PgPool, ctx: &AuthContext, module_id: Uuid, prerequisite_module_id: Uuid) -> Result<(), AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    sqlx::query!(r#"delete from module_prerequisites where module_id = $1 and prerequisite_module_id = $2"#, module_id, prerequisite_module_id)
        .execute(pool)
        .await?;
    Ok(())
}
