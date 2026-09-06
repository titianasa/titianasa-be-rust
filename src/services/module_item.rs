use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::module::{CreateModuleItemRequest, ReorderModuleItemsRequest, UpdateModuleItemMetaRequest};
use crate::models::responses::module::{
    ModuleItemBlockResponse, ModuleItemDetailResponse, ModuleItemNode, ModuleItemStatusResponse, ModuleItemTreeResponse, SpeakingPromptResponse,
};
use crate::services::content_block::{self, NewContentBlock};
use crate::services::content_qa;
use crate::services::paste_normalizer::{self, SourceFormat};
use crate::services::permissions::{is_allowed, require_permission, Action, Resource};
use crate::services::{alm_parser, publish_flow};

// Phase 31 (P31-002) — the SECOND, independent tree: content items
// WITHIN exactly one module (section/item, depth<=4, i.e. max 5
// levels). Direct successor of content.rs's lesson functions — same
// permission tier, same ADR-0008 (published = immutable) and P2-011
// (Grammar Constitution) rules, just retargeted at module_items instead
// of lessons/units/levels/curricula.

pub struct ContentBlockRow {
    pub id: Uuid,
    pub r#type: String,
    pub order_index: i32,
    pub data: serde_json::Value,
    pub raw_source: Option<String>,
}

struct ItemRow {
    id: Uuid,
    module_id: Uuid,
    parent_id: Option<Uuid>,
    node_type: String,
    depth: i32,
    title: String,
    content_type: Option<String>,
    status: String,
    qa_report: Option<serde_json::Value>,
    generated_by: String,
}

const CONTENT_TYPES: [&str; 6] = ["learn", "practice", "speaking", "writing", "review", "assessment"];
const MAX_DEPTH: i32 = 4;

// Exposed so content_qa.rs can reuse it without a duplicate query, same
// as the old content.rs::find_content_blocks.
pub async fn find_content_blocks(pool: &PgPool, item_id: Uuid) -> Result<Vec<ContentBlockRow>, AppError> {
    let rows = sqlx::query_as!(
        ContentBlockRow,
        r#"select id, type, order_index, data, raw_source from content_blocks
           where item_id = $1 order by order_index asc"#,
        item_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ItemRow>, AppError> {
    let row = sqlx::query_as!(
        ItemRow,
        r#"select id, module_id, parent_id, node_type, depth, title, content_type, status, qa_report, generated_by
           from module_items where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

fn to_status_response(item: &ItemRow) -> ModuleItemStatusResponse {
    ModuleItemStatusResponse { id: item.id, status: item.status.clone(), qa_report: item.qa_report.clone() }
}

// GET /modules/{id}/items — the full nested tree in one response, since
// depth<=5 keeps it small (no pagination needed).
pub async fn get_tree(pool: &PgPool, module_id: Uuid) -> Result<ModuleItemTreeResponse, AppError> {
    struct FlatRow {
        id: Uuid,
        parent_id: Option<Uuid>,
        node_type: String,
        title: String,
        order_index: i32,
        content_type: Option<String>,
        status: String,
        generated_by: String,
    }
    let rows = sqlx::query_as!(
        FlatRow,
        r#"select id, parent_id, node_type, title, order_index, content_type, status, generated_by
           from module_items where module_id = $1 order by order_index asc"#,
        module_id,
    )
    .fetch_all(pool)
    .await?;

    let mut children_by_parent: std::collections::HashMap<Option<Uuid>, Vec<FlatRow>> = std::collections::HashMap::new();
    for row in rows {
        children_by_parent.entry(row.parent_id).or_default().push(row);
    }

    fn build(parent_id: Option<Uuid>, children_by_parent: &mut std::collections::HashMap<Option<Uuid>, Vec<FlatRow>>) -> Vec<ModuleItemNode> {
        let Some(rows) = children_by_parent.remove(&parent_id) else { return vec![] };
        rows.into_iter()
            .map(|r| {
                let children = build(Some(r.id), children_by_parent);
                ModuleItemNode {
                    id: r.id,
                    parent_id: r.parent_id,
                    node_type: r.node_type,
                    title: r.title,
                    order_index: r.order_index,
                    content_type: r.content_type,
                    status: r.status,
                    generated_by: r.generated_by,
                    children,
                }
            })
            .collect()
    }

    Ok(ModuleItemTreeResponse { items: build(None, &mut children_by_parent) })
}

fn parse_source_format(format: &str) -> Result<SourceFormat, AppError> {
    match format {
        "markdown" => Ok(SourceFormat::Markdown),
        "html" => Ok(SourceFormat::Html),
        other => Err(AppError::UnprocessableEntity("invalid_source_format", format!(r#"format "{other}" must be "markdown" or "html""#))),
    }
}

async fn parse_and_write_blocks(pool: &PgPool, item_id: Uuid, content: &str, format: &str) -> Result<(), AppError> {
    let source_format = parse_source_format(format)?;
    let normalized = paste_normalizer::normalize(content, source_format);
    let parsed = alm_parser::parse(&normalized)?;

    let blocks: Vec<NewContentBlock> = parsed
        .into_iter()
        .enumerate()
        .map(|(i, b)| NewContentBlock { r#type: b.r#type, order_index: i as i32, data: b.data, raw_source: Some(b.raw_source) })
        .collect();

    content_block::validate_and_replace_blocks(pool, item_id, blocks).await
}

// POST /modules/{id}/items — create a section (organizational node, no
// content) or an item (leaf, optionally with content parsed immediately
// — same "never half-created" rule as the old create_lesson).
pub async fn create(pool: &PgPool, ctx: &AuthContext, module_id: Uuid, req: CreateModuleItemRequest) -> Result<ModuleItemStatusResponse, AppError> {
    create_with_provenance(pool, ctx, module_id, req, "human").await
}

// Same as `create`, but lets a server-side caller (ai_content.rs)
// stamp generated_by='ai' — deliberately not a CreateModuleItemRequest
// field, since that DTO is also the public POST /modules/{id}/items
// body and a client shouldn't be able to self-report AI provenance.
pub async fn create_with_provenance(pool: &PgPool, ctx: &AuthContext, module_id: Uuid, req: CreateModuleItemRequest, generated_by: &str) -> Result<ModuleItemStatusResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;

    if req.node_type != "section" && req.node_type != "item" {
        return Err(AppError::UnprocessableEntity("invalid_node_type", r#"node_type must be "section" or "item""#.to_string()));
    }
    if req.node_type == "item" {
        let Some(content_type) = &req.content_type else {
            return Err(AppError::UnprocessableEntity("content_type_required", "content_type is required for an item".to_string()));
        };
        if !CONTENT_TYPES.contains(&content_type.as_str()) {
            return Err(AppError::UnprocessableEntity(
                "invalid_content_type",
                format!(r#"content_type "{content_type}" must be one of [{}]"#, CONTENT_TYPES.join(", ")),
            ));
        }
    }

    let depth = if let Some(parent_id) = req.parent_id {
        let parent = find_by_id(pool, parent_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
        if parent.module_id != module_id {
            return Err(AppError::UnprocessableEntity("parent_in_different_module", "parent_id must belong to the same module".to_string()));
        }
        if parent.depth >= MAX_DEPTH {
            return Err(AppError::UnprocessableEntity("max_depth_exceeded", format!("module item tree is limited to {} levels", MAX_DEPTH + 1)));
        }
        parent.depth + 1
    } else {
        0
    };

    let row = sqlx::query_as!(
        ItemRow,
        r#"insert into module_items (module_id, parent_id, node_type, depth, title, content_type, generated_by)
           values ($1, $2, $3, $4, $5, $6, $7)
           returning id, module_id, parent_id, node_type, depth, title, content_type, status, qa_report, generated_by"#,
        module_id,
        req.parent_id,
        req.node_type,
        depth,
        req.title,
        req.content_type,
        generated_by,
    )
    .fetch_one(pool)
    .await?;

    for concept_id in req.concept_ids.unwrap_or_default() {
        sqlx::query!(r#"insert into module_item_concepts (item_id, concept_id, weight) values ($1, $2, 1.0)"#, row.id, concept_id).execute(pool).await?;
    }

    if let (Some(content), Some(format)) = (&req.content, &req.format) {
        parse_and_write_blocks(pool, row.id, content, format).await?;
    }

    Ok(to_status_response(&row))
}

// GET /module-items/{id} — unpublished items 403 for everyone except
// curriculum_developer/reviewer/admin roles.
pub async fn get_detail(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<ModuleItemDetailResponse, AppError> {
    let item = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    if item.status != "published" && !is_allowed(ctx.role.as_deref(), Resource::ModuleItem, Action::ViewUnpublished) {
        return Err(AppError::ForbiddenWithCode("module_item_not_published"));
    }

    let blocks = find_content_blocks(pool, item_id).await?;
    Ok(ModuleItemDetailResponse {
        id: item.id,
        module_id: item.module_id,
        parent_id: item.parent_id,
        node_type: item.node_type,
        title: item.title,
        content_type: item.content_type,
        status: item.status,
        qa_report: item.qa_report,
        generated_by: item.generated_by,
        blocks: blocks.into_iter().map(|b| ModuleItemBlockResponse { id: b.id, r#type: b.r#type, order_index: b.order_index, data: b.data, raw_source: b.raw_source }).collect(),
    })
}

// GET /module-items/{id}/speaking-prompt-audio — content lookup only
// (handler owns the synthesize_speech call). Same published gate as
// get_detail.
pub async fn get_speaking_prompt_text(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<SpeakingPromptResponse, AppError> {
    let item = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    if item.status != "published" && !is_allowed(ctx.role.as_deref(), Resource::ModuleItem, Action::ViewUnpublished) {
        return Err(AppError::ForbiddenWithCode("module_item_not_published"));
    }

    let blocks = find_content_blocks(pool, item_id).await?;
    let prompt_block = blocks.into_iter().find(|b| b.r#type == "speaking_prompt").ok_or(AppError::NotFound("speaking_prompt_not_found"))?;

    let text = prompt_block.data.get("text").and_then(|v| v.as_str()).filter(|t| !t.is_empty());
    let Some(text) = text else {
        return Err(AppError::Internal(anyhow::anyhow!("speaking_prompt block {} has no text", prompt_block.id)));
    };
    Ok(SpeakingPromptResponse { text: text.to_string() })
}

// PATCH /module-items/{id} — rename/move within the same module. A
// section/item may only be moved under another node in the SAME
// module_id (the item tree is scoped to one module).
pub async fn update_meta(pool: &PgPool, ctx: &AuthContext, id: Uuid, req: UpdateModuleItemMetaRequest) -> Result<ModuleItemStatusResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    let existing = find_by_id(pool, id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;

    let (new_parent_id, new_depth) = if let Some(parent_id) = req.parent_id {
        if parent_id == id {
            return Err(AppError::UnprocessableEntity("invalid_move", "an item cannot be moved into itself".to_string()));
        }
        let parent = find_by_id(pool, parent_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
        if parent.module_id != existing.module_id {
            return Err(AppError::UnprocessableEntity("parent_in_different_module", "parent_id must belong to the same module".to_string()));
        }
        if parent.depth >= MAX_DEPTH {
            return Err(AppError::UnprocessableEntity("max_depth_exceeded", format!("module item tree is limited to {} levels", MAX_DEPTH + 1)));
        }
        let descendants = find_descendant_ids(pool, id).await?;
        if descendants.contains(&parent_id) {
            return Err(AppError::UnprocessableEntity("invalid_move", "an item cannot be moved into its own descendant".to_string()));
        }
        (Some(parent_id), parent.depth + 1)
    } else {
        (existing.parent_id, existing.depth)
    };

    let title = req.title.unwrap_or(existing.title);

    let row = sqlx::query_as!(
        ItemRow,
        r#"update module_items set title = $2, parent_id = $3, depth = $4, updated_at = now() where id = $1
           returning id, module_id, parent_id, node_type, depth, title, content_type, status, qa_report, generated_by"#,
        id,
        title,
        new_parent_id,
        new_depth,
    )
    .fetch_one(pool)
    .await?;
    Ok(to_status_response(&row))
}

async fn find_descendant_ids(pool: &PgPool, id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"with recursive descendants as (
             select i.id from module_items i where i.parent_id = $1
             union all
             select i.id from module_items i join descendants d on i.parent_id = d.id
           )
           select id as "id!" from descendants"#,
        id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// PUT /module-items/{id} — re-parses ALM/HTML and replaces
// content_blocks wholesale. ADR-0008: a published item can never be
// edited in place.
pub async fn update_content(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, content: &str, format: &str) -> Result<ModuleItemStatusResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    let item = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    if item.status == "published" {
        return Err(AppError::UnprocessableEntity(
            "cannot_edit_published_content",
            "a published module item cannot be edited in place — see ADR-0008".to_string(),
        ));
    }

    parse_and_write_blocks(pool, item_id, content, format).await?;
    Ok(to_status_response(&item))
}

// POST /module-items/{id}/submit-review. P2-011: an item linked to a
// type='grammar' concept must pass the Constitution check before it's
// allowed into the review queue. Once that hard gate passes, the QA
// Agent runs too — a QA finding never blocks this transition.
pub async fn submit_for_review(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<ModuleItemStatusResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::SubmitReview)?;
    let item = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    publish_flow::validate_submit_for_review(&item.status)?;

    let has_grammar_concept = sqlx::query_scalar!(
        r#"select c.id from module_item_concepts ic inner join concepts c on c.id = ic.concept_id
           where ic.item_id = $1 and c.type = 'grammar' limit 1"#,
        item_id,
    )
    .fetch_optional(pool)
    .await?
    .is_some();

    if has_grammar_concept {
        let blocks = find_content_blocks(pool, item_id).await?;
        let typed: Vec<crate::services::curriculum_constitution::TypedBlock> =
            blocks.iter().map(|b| crate::services::curriculum_constitution::TypedBlock { r#type: b.r#type.clone(), data: b.data.clone() }).collect();
        crate::services::curriculum_constitution::validate_grammar_lesson(&typed)?;
    }

    let qa_report = content_qa::run_item_qa(pool, item_id).await?;
    let qa_report_json = serde_json::to_value(&qa_report).unwrap();

    let row = sqlx::query_as!(
        ItemRow,
        r#"update module_items set status = 'in_review', qa_report = $2, updated_at = now() where id = $1
           returning id, module_id, parent_id, node_type, depth, title, content_type, status, qa_report, generated_by"#,
        item_id,
        qa_report_json,
    )
    .fetch_one(pool)
    .await?;

    Ok(to_status_response(&row))
}

async fn update_status(pool: &PgPool, id: Uuid, status: &str) -> Result<ItemRow, AppError> {
    let row = sqlx::query_as!(
        ItemRow,
        r#"update module_items set status = $2, updated_at = now() where id = $1
           returning id, module_id, parent_id, node_type, depth, title, content_type, status, qa_report, generated_by"#,
        id,
        status,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// POST /module-items/{id}/publish.
pub async fn publish(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<ModuleItemStatusResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Publish)?;
    let item = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    publish_flow::validate_publish(&item.status)?;
    let row = update_status(pool, item_id, "published").await?;
    Ok(to_status_response(&row))
}

// POST /module-items/{id}/reject — sends an in_review item back to draft.
pub async fn reject(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<ModuleItemStatusResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Publish)?;
    let item = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    publish_flow::validate_reject(&item.status)?;
    let row = update_status(pool, item_id, "draft").await?;
    Ok(to_status_response(&row))
}

// POST /module-items/reorder
pub async fn reorder(pool: &PgPool, ctx: &AuthContext, req: ReorderModuleItemsRequest) -> Result<(), AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    let mut tx = pool.begin().await?;
    for (index, id) in req.ordered_ids.iter().enumerate() {
        sqlx::query!(
            r#"update module_items set order_index = $2, updated_at = now()
               where id = $1 and module_id = $3 and parent_id is not distinct from $4"#,
            id,
            index as i32,
            req.module_id,
            req.parent_id,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
