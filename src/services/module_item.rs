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
use crate::services::{alm_parser, lesson_plan, publish_flow};

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
    quiz_config: Option<serde_json::Value>,
    status: String,
    qa_report: Option<serde_json::Value>,
    generated_by: String,
    // Migration 0043 — overrides the item's module's subject when set.
    // Only one dedicated endpoint (update_subject) ever writes this;
    // every other ItemRow query here selects it read-only, unchanged.
    subject_id: Option<Uuid>,
}

// Phase 37 — collapsed from the old 6 English-learning-specific values
// to 2 generic ones. See migrations/0038's header for the backfill
// mapping and quiz_subtype.rs for the ~40-subtype registry `quiz`
// items compose their `quiz_config` from.
const CONTENT_TYPES: [&str; 2] = ["article", "quiz"];
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

// Exposed for handlers/collab_ws.rs's ADR-0008 gate (a published/
// archived item's collab socket refuses new connections — v1 is
// authoring-only, not live-viewing of published content) without
// pulling in the full ItemRow/get_detail machinery.
pub async fn find_status(pool: &PgPool, item_id: Uuid) -> Result<Option<String>, AppError> {
    let status = sqlx::query_scalar!(r#"select status from module_items where id = $1"#, item_id).fetch_optional(pool).await?;
    Ok(status)
}

async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ItemRow>, AppError> {
    let row = sqlx::query_as!(
        ItemRow,
        r#"select id, module_id, parent_id, node_type, depth, title, content_type, quiz_config, status, qa_report, generated_by, subject_id
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
/// `learner` — set for someone taking the module (not authoring it):
/// every node then carries that learner's locks and completion.
pub async fn get_tree(pool: &PgPool, module_id: Uuid, learner: Option<Uuid>) -> Result<ModuleItemTreeResponse, AppError> {
    // Migration 0042 — a reference row carries no items of its own; the
    // learner sees the library module's content through it.
    let module_id = crate::services::module::resolve_content_module(pool, module_id).await?;
    let learner_state = match learner {
        Some(user_id) => Some(crate::services::item_guard::learner_state(pool, module_id, user_id).await?),
        None => None,
    };
    let empty_locks = std::collections::HashMap::new();
    let empty_done = std::collections::HashSet::new();
    let locks = learner_state.as_ref().map(|s| &s.locks).unwrap_or(&empty_locks);
    let completed = learner_state.as_ref().map(|s| &s.completed).unwrap_or(&empty_done);
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

    fn build(
        parent_id: Option<Uuid>,
        children_by_parent: &mut std::collections::HashMap<Option<Uuid>, Vec<FlatRow>>,
        locks: &std::collections::HashMap<Uuid, String>,
        completed: &std::collections::HashSet<Uuid>,
    ) -> Vec<ModuleItemNode> {
        let Some(rows) = children_by_parent.remove(&parent_id) else { return vec![] };
        rows.into_iter()
            .map(|r| {
                let children = build(Some(r.id), children_by_parent, locks, completed);
                ModuleItemNode {
                    id: r.id,
                    parent_id: r.parent_id,
                    node_type: r.node_type,
                    title: r.title,
                    order_index: r.order_index,
                    content_type: r.content_type,
                    status: r.status,
                    generated_by: r.generated_by,
                    lock_reason: locks.get(&r.id).cloned(),
                    completed: completed.contains(&r.id),
                    children,
                }
            })
            .collect()
    }

    Ok(ModuleItemTreeResponse { items: build(None, &mut children_by_parent, locks, completed) })
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

    // Migration 0042 — a reference row is a pointer, not a container.
    // Items written here would be invisible forever, since get_tree
    // resolves reads to the source. Refuse loudly instead of silently
    // losing the author's work; redirecting the write would be worse
    // still, quietly mutating the library for every other path too.
    if crate::services::module::resolve_content_module(pool, module_id).await? != module_id {
        return Err(AppError::UnprocessableEntity(
            "module_is_reference",
            "this module is a reference to a library module; add the item to the source module instead".to_string(),
        ));
    }

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
        r#"insert into module_items (module_id, parent_id, node_type, depth, title, content_type, quiz_config, generated_by, subject_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
           returning id, module_id, parent_id, node_type, depth, title, content_type, quiz_config, status, qa_report, generated_by, subject_id"#,
        module_id,
        req.parent_id,
        req.node_type,
        depth,
        req.title,
        req.content_type,
        req.quiz_config,
        generated_by,
        req.subject_id,
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
    // Phase 31 (P31-008) — a collaborator explicitly shared onto this
    // item (viewer or editor) can see it even without an author-tier
    // role, same as the collab WS's own gate.
    if item.status != "published"
        && !is_allowed(ctx.role.as_deref(), Resource::ModuleItem, Action::ViewUnpublished)
        && !crate::services::resource_share::has_module_item_grant(pool, ctx.user_id, ctx.role.as_deref(), item_id, "viewer").await?
    {
        return Err(AppError::ForbiddenWithCode("module_item_not_published"));
    }

    // Access gates ("Aturan Akses & Guard") bind learners only — an author
    // must still be able to open every item they are building.
    if item.node_type == "item" && !is_allowed(ctx.role.as_deref(), Resource::ModuleItem, Action::ViewUnpublished) {
        if let Some(reason) = crate::services::item_guard::lock_reason(pool, item_id, ctx.user_id).await? {
            return Err(AppError::UnprocessableEntity("item_locked", reason));
        }
    }

    let blocks = find_content_blocks(pool, item_id).await?;
    // Read separately rather than widening ItemRow, which a dozen
    // queries here select into and none of the others need these.
    let extra = sqlx::query!(r#"select lesson_plan, guard_config, attendance_guard from module_items where id = $1"#, item_id).fetch_one(pool).await?;
    Ok(ModuleItemDetailResponse {
        lesson_plan: extra.lesson_plan,
        guard_config: extra.guard_config,
        attendance_guard: extra.attendance_guard,
        id: item.id,
        module_id: item.module_id,
        parent_id: item.parent_id,
        node_type: item.node_type,
        title: item.title,
        content_type: item.content_type,
        quiz_config: item.quiz_config,
        status: item.status,
        qa_report: item.qa_report,
        generated_by: item.generated_by,
        subject_id: item.subject_id,
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
    can_edit_item(pool, ctx, id).await?;
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
           returning id, module_id, parent_id, node_type, depth, title, content_type, quiz_config, status, qa_report, generated_by, subject_id"#,
        id,
        title,
        new_parent_id,
        new_depth,
    )
    .fetch_one(pool)
    .await?;
    Ok(to_status_response(&row))
}

// PATCH /module-items/{id}/subject — deliberately its OWN endpoint
// rather than a field on update_meta: that function treats an absent
// field as "leave unchanged", which is right for title/parent_id but
// wrong here — JSON can't distinguish "subject_id omitted" from
// "subject_id explicitly cleared" on a plain Option<Uuid>, and a rename
// or move must never silently blow away an override as a side effect.
// This endpoint instead always applies exactly what's sent: Some(id) to
// set/change the override, None to clear it back to "inherit from the
// module" (mirrors quiz-config.rs's same one-concern-per-endpoint idiom).
pub async fn update_subject(pool: &PgPool, ctx: &AuthContext, id: Uuid, subject_id: Option<Uuid>) -> Result<ModuleItemStatusResponse, AppError> {
    can_edit_item(pool, ctx, id).await?;
    find_by_id(pool, id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;

    let row = sqlx::query_as!(
        ItemRow,
        r#"update module_items set subject_id = $2, updated_at = now() where id = $1
           returning id, module_id, parent_id, node_type, depth, title, content_type, quiz_config, status, qa_report, generated_by, subject_id"#,
        id,
        subject_id,
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
// Phase 31 (P31-008) — a real author (Create-tier role) always passes;
// otherwise an explicit editor-level share grant on this item also
// passes (a collaborator invited onto one item, without being promoted
// to curriculum_developer org-wide). Mirrors the collab WS's own gate.
pub(crate) async fn can_edit_item(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<(), AppError> {
    if is_allowed(ctx.role.as_deref(), Resource::ModuleItem, Action::Create) {
        return Ok(());
    }
    if crate::services::resource_share::has_module_item_grant(pool, ctx.user_id, ctx.role.as_deref(), item_id, "editor").await? {
        return Ok(());
    }
    Err(AppError::Forbidden)
}

pub async fn update_content(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, content: &str, format: &str) -> Result<ModuleItemStatusResponse, AppError> {
    can_edit_item(pool, ctx, item_id).await?;
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

// PATCH /module-items/{id}/quiz-config — the Quiz Builder's save
// action. Same edit gate and ADR-0008 published-immutable rule as
// update_content; only valid on a `content_type="quiz"` item.
pub async fn update_quiz_config(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, quiz_config: serde_json::Value) -> Result<ModuleItemStatusResponse, AppError> {
    can_edit_item(pool, ctx, item_id).await?;
    let item = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    if item.status == "published" {
        return Err(AppError::UnprocessableEntity(
            "cannot_edit_published_content",
            "a published module item cannot be edited in place — see ADR-0008".to_string(),
        ));
    }
    if item.content_type.as_deref() != Some("quiz") {
        return Err(AppError::UnprocessableEntity("not_a_quiz_item", "quiz_config can only be set on a content_type=\"quiz\" item".to_string()));
    }

    crate::services::quiz_config_schema::validate_structure(&quiz_config)?;

    let row = sqlx::query_as!(
        ItemRow,
        r#"update module_items set quiz_config = $2, updated_at = now() where id = $1
           returning id, module_id, parent_id, node_type, depth, title, content_type, quiz_config, status, qa_report, generated_by, subject_id"#,
        item_id,
        quiz_config,
    )
    .fetch_one(pool)
    .await?;
    Ok(to_status_response(&row))
}

// PATCH /module-items/{id}/lesson-plan — the Modul Belajar editor's
// save. Same edit gate and ADR-0008 rule as update_content. The plan is
// the source of truth; content_blocks are re-projected from it in the
// same call, so every reader of blocks (QA, search) stays current.
pub async fn update_lesson_plan(
    pool: &PgPool,
    ctx: &AuthContext,
    item_id: Uuid,
    raw: serde_json::Value,
) -> Result<crate::models::responses::module::LessonPlanSaveResponse, AppError> {
    can_edit_item(pool, ctx, item_id).await?;
    let item = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    if item.status == "published" {
        return Err(AppError::UnprocessableEntity(
            "cannot_edit_published_content",
            "a published module item cannot be edited in place — see ADR-0008".to_string(),
        ));
    }
    if item.content_type.as_deref() != Some("article") {
        return Err(AppError::UnprocessableEntity("not_an_article", "lesson_plan can only be set on a content_type=\"article\" item".to_string()));
    }

    let plan = lesson_plan::normalize(lesson_plan::parse(&raw)?)?;
    parse_and_write_blocks(pool, item_id, &lesson_plan::to_alm(&plan), "markdown").await?;
    let value = serde_json::to_value(&plan).map_err(|e| AppError::Internal(e.into()))?;
    sqlx::query!(r#"update module_items set lesson_plan = $2, updated_at = now() where id = $1"#, item_id, value).execute(pool).await?;

    Ok(crate::models::responses::module::LessonPlanSaveResponse { id: item.id, status: item.status, qa_report: item.qa_report, lesson_plan: value })
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
           returning id, module_id, parent_id, node_type, depth, title, content_type, quiz_config, status, qa_report, generated_by, subject_id"#,
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
           returning id, module_id, parent_id, node_type, depth, title, content_type, quiz_config, status, qa_report, generated_by, subject_id"#,
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

// DELETE /module-items/{id}. A section's whole subtree goes with it —
// module_items.parent_id cascades on delete, but content_blocks and
// resource_shares don't reference module_items with a cascading FK, so
// those are cleared explicitly for every id in the subtree first. Real
// student usage (attempts/canvas_sessions/completion overrides) blocks
// the delete outright rather than silently destroying that history.
pub async fn delete(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<(), AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?;

    let mut ids = find_descendant_ids(pool, item_id).await?;
    ids.push(item_id);

    for id in &ids {
        let has_history = sqlx::query_scalar!(
            r#"select exists(
                 select 1 from attempts where item_id = $1
                 union all select 1 from canvas_sessions where item_id = $1
                 union all select 1 from item_completion_overrides where item_id = $1
               ) as "has_history!""#,
            id,
        )
        .fetch_one(pool)
        .await?;
        if has_history {
            return Err(AppError::UnprocessableEntity(
                "item_has_history",
                "item ini (atau isi di dalamnya) sudah punya riwayat pengerjaan siswa dan tidak bisa dihapus".to_string(),
            ));
        }
    }

    let mut tx = pool.begin().await?;
    for id in &ids {
        sqlx::query!(r#"delete from content_blocks where item_id = $1"#, id).execute(&mut *tx).await?;
        sqlx::query!(r#"delete from resource_shares where resource_type = 'module_item' and resource_id = $1"#, id).execute(&mut *tx).await?;
    }
    sqlx::query!(r#"delete from module_items where id = $1"#, item_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

struct DuplicateRow {
    id: Uuid,
    parent_id: Option<Uuid>,
    node_type: String,
    depth: i32,
    title: String,
    order_index: i32,
    content_type: Option<String>,
    quiz_config: Option<serde_json::Value>,
    lesson_plan: Option<serde_json::Value>,
    generated_by: String,
}

// POST /module-items/{id}/duplicate — clones a node as a new sibling
// right after the original; a section brings its whole subtree along.
// The copy always starts life as a fresh draft (no qa_report/status
// carried over) even when the original is published/in_review — ADR-
// 0008 only protects the ORIGINAL from being edited in place, a
// duplicate is a brand new item with no history yet.
pub async fn duplicate(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<ModuleItemStatusResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    let original = sqlx::query_as!(
        DuplicateRow,
        r#"select id, parent_id, node_type, depth, title, order_index, content_type, quiz_config, lesson_plan, generated_by
           from module_items where id = $1"#,
        item_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("module_item_not_found"))?;

    let descendant_ids = find_descendant_ids(pool, item_id).await?;
    let mut rows = vec![original];
    for id in descendant_ids {
        if let Some(row) = sqlx::query_as!(
            DuplicateRow,
            r#"select id, parent_id, node_type, depth, title, order_index, content_type, quiz_config, lesson_plan, generated_by
               from module_items where id = $1"#,
            id,
        )
        .fetch_optional(pool)
        .await?
        {
            rows.push(row);
        }
    }
    rows.sort_by_key(|r| r.depth);

    // `DuplicateRow` doesn't carry module_id (every row in the subtree
    // shares the same one anyway), so grab it separately.
    let module_id = find_by_id(pool, item_id).await?.ok_or(AppError::NotFound("module_item_not_found"))?.module_id;

    let root = &rows[0];
    // The copy lands as the very next sibling, not appended to the end
    // of the list — everything from that slot onward shifts down one.
    let insert_at = root.order_index + 1;
    let root_id = root.id;
    let mut id_map: std::collections::HashMap<Uuid, Uuid> = std::collections::HashMap::new();
    let mut tx = pool.begin().await?;

    sqlx::query!(
        r#"update module_items set order_index = order_index + 1, updated_at = now()
           where module_id = $1 and parent_id is not distinct from $2 and order_index >= $3"#,
        module_id,
        root.parent_id,
        insert_at,
    )
    .execute(&mut *tx)
    .await?;

    for row in &rows {
        let new_id = Uuid::new_v4();
        id_map.insert(row.id, new_id);
        let new_parent_id = if row.id == root_id { root.parent_id } else { row.parent_id.and_then(|p| id_map.get(&p).copied()) };
        let title = if row.id == root_id { format!("{} (salinan)", row.title) } else { row.title.clone() };
        let order_index = if row.id == root_id { insert_at } else { row.order_index };

        sqlx::query!(
            r#"insert into module_items (id, module_id, parent_id, node_type, depth, title, order_index, content_type, quiz_config, lesson_plan, status, generated_by)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $11, 'draft', $10)"#,
            new_id,
            module_id,
            new_parent_id,
            row.node_type,
            row.depth,
            title,
            order_index,
            row.content_type,
            row.quiz_config,
            row.generated_by,
            row.lesson_plan,
        )
        .execute(&mut *tx)
        .await?;

        for block in find_content_blocks(pool, row.id).await? {
            sqlx::query!(
                r#"insert into content_blocks (item_id, type, order_index, data, raw_source) values ($1, $2, $3, $4, $5)"#,
                new_id,
                block.r#type,
                block.order_index,
                block.data,
                block.raw_source,
            )
            .execute(&mut *tx)
            .await?;
        }

        let concepts = sqlx::query!(r#"select concept_id, weight from module_item_concepts where item_id = $1"#, row.id).fetch_all(pool).await?;
        for c in concepts {
            sqlx::query!(r#"insert into module_item_concepts (item_id, concept_id, weight) values ($1, $2, $3)"#, new_id, c.concept_id, c.weight)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;

    let new_root = find_by_id(pool, *id_map.get(&root_id).unwrap()).await?.ok_or(AppError::NotFound("module_item_not_found"))?;
    Ok(to_status_response(&new_root))
}
