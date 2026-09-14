// Where a piece of content sits in the curriculum: subject → folders
// (Tahap, domain…) → topik (the module) → bab (the section node) → item.
//
// Two consumers need the same answer and must never disagree about it:
// content report tickets (so an admin sees "Matematika › Tahap 1 ›
// Bilangan Cacah › Nilai Tempat" instead of a bare uuid) and the answer
// facts behind every heatmap (so a correct answer is counted under the
// same bab its question is shown in).
//
// Folder depth is not fixed across subjects — Matematika has a domain
// folder between Tahap and topik, others may not — so the ancestry is
// kept as a PATH (root → module), and a report or heatmap picks the
// level it wants from it instead of this file hard-coding "level 2 is
// Tahap".

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize)]
pub struct PathNode {
    pub id: Uuid,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemContext {
    pub item_id: Uuid,
    pub item_title: String,
    pub module_id: Uuid,
    /// Folder ancestors from the root down to AND INCLUDING the module
    /// (topik) itself.
    pub path: Vec<PathNode>,
    /// The nearest `section` node above the item — the bab.
    pub bab: Option<PathNode>,
    /// Item override → module → nearest folder that sets one.
    pub subject_id: Option<Uuid>,
    pub subject_name: Option<String>,
}

pub async fn resolve(pool: &PgPool, item_id: Uuid) -> Result<Option<ItemContext>, AppError> {
    let Some(item) = sqlx::query!(r#"select id, module_id, title, subject_id from module_items where id = $1"#, item_id).fetch_optional(pool).await? else {
        return Ok(None);
    };

    let bab = sqlx::query!(
        r#"with recursive up as (
             select id, parent_id, node_type, title, 0 as depth from module_items where id = $1
             union all
             select p.id, p.parent_id, p.node_type, p.title, up.depth + 1 from module_items p join up on p.id = up.parent_id
           )
           select id as "id!", title as "title!" from up where node_type = 'section' and depth > 0 order by depth limit 1"#,
        item_id,
    )
    .fetch_optional(pool)
    .await?
    .map(|r| PathNode { id: r.id, title: r.title });

    let chain = sqlx::query!(
        r#"with recursive up as (
             select id, parent_id, title, subject_id, 0 as depth from modules where id = $1
             union all
             select p.id, p.parent_id, p.title, p.subject_id, up.depth + 1 from modules p join up on p.id = up.parent_id
           )
           select id as "id!", title as "title!", subject_id, depth as "depth!" from up order by depth desc"#,
        item.module_id,
    )
    .fetch_all(pool)
    .await?;

    // Deepest (closest to the item) subject wins.
    let subject_id = item.subject_id.or_else(|| chain.iter().rev().find_map(|n| n.subject_id));
    let subject_name = match subject_id {
        Some(id) => sqlx::query_scalar!(r#"select name from subjects where id = $1"#, id).fetch_optional(pool).await?,
        None => None,
    };

    Ok(Some(ItemContext {
        item_id,
        item_title: item.title,
        module_id: item.module_id,
        path: chain.into_iter().map(|n| PathNode { id: n.id, title: n.title }).collect(),
        bab,
        subject_id,
        subject_name,
    }))
}
