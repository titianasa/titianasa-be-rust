use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};

// Curriculum mapping layer (migration 0039). A label answers "where
// does this topic appear" — jenjang=SMA-12, ujian=utbk-pm, cefr=B1 —
// and, read the other way, assembles a learning path from the master
// library without duplicating modules into it.

#[derive(Debug, serde::Serialize)]
pub struct ModuleLabel {
    pub id: Uuid,
    pub module_id: Uuid,
    pub kind: String,
    pub value: String,
    pub source: String,
}

// Kinds are a curated vocabulary rather than a DB CHECK: an autonomous
// curriculum agent will need to coin new axes (say `profesi` or
// `olimpiade`) without a migration, so the list below is guidance for
// callers, not a gate. Same reasoning as block_schema/question_schema's
// code-side registries.
pub const KNOWN_KINDS: [&str; 6] = ["jenjang", "kurikulum", "ujian", "cefr", "cambridge", "tema"];

fn validate(kind: &str, value: &str) -> Result<(), AppError> {
    if kind.trim().is_empty() {
        return Err(AppError::UnprocessableEntity("invalid_label", "kind tidak boleh kosong".to_string()));
    }
    if value.trim().is_empty() {
        return Err(AppError::UnprocessableEntity("invalid_label", "value tidak boleh kosong".to_string()));
    }
    Ok(())
}

// POST /modules/{id}/labels — idempotent: re-labelling an already
// labelled module returns the existing row rather than erroring, so a
// re-run of a seeding/agent pass is safe.
pub async fn attach(pool: &PgPool, ctx: &AuthContext, module_id: Uuid, kind: &str, value: &str, source: &str) -> Result<ModuleLabel, AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    validate(kind, value)?;
    if source != "human" && source != "ai" {
        return Err(AppError::UnprocessableEntity("invalid_source", r#"source harus "human" atau "ai""#.to_string()));
    }

    let row = sqlx::query_as!(
        ModuleLabel,
        r#"insert into module_labels (module_id, kind, value, source) values ($1, $2, $3, $4)
           on conflict (module_id, kind, value) do update set source = module_labels.source
           returning id, module_id, kind, value, source"#,
        module_id,
        kind,
        value,
        source,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /modules/{id}/labels
pub async fn list_for_module(pool: &PgPool, module_id: Uuid) -> Result<Vec<ModuleLabel>, AppError> {
    let rows = sqlx::query_as!(
        ModuleLabel,
        r#"select id, module_id, kind, value, source from module_labels
           where module_id = $1 order by kind asc, value asc"#,
        module_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// DELETE /modules/{id}/labels/{label_id}
pub async fn detach(pool: &PgPool, ctx: &AuthContext, module_id: Uuid, label_id: Uuid) -> Result<(), AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    sqlx::query!(r#"delete from module_labels where id = $1 and module_id = $2"#, label_id, module_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct LabeledModule {
    pub module_id: Uuid,
    pub title: String,
    pub is_folder: bool,
    /// Carried so a caller assembling a path by label can build a
    /// reference row without a second lookup per topic.
    pub subject_id: Option<Uuid>,
    pub status: String,
    /// Breadcrumb from the library root, e.g.
    /// "Semua Mata Pelajaran / Matematika / Tahap 3 / Turunan".
    pub path: String,
}

// GET /modules/by-label?kind=&value= — the reverse lookup that lets a
// learning path be assembled from the library instead of copied into
// it. Path is resolved server-side so a caller can render a result
// without walking the tree itself.
pub async fn find_by_label(pool: &PgPool, kind: &str, value: &str) -> Result<Vec<LabeledModule>, AppError> {
    let rows = sqlx::query!(
        r#"with recursive ancestry as (
             select m.id, m.parent_id, m.title, m.is_folder, m.status, m.subject_id, m.title as path
             from modules m
             where m.parent_id is null
             union all
             select c.id, c.parent_id, c.title, c.is_folder, c.status, c.subject_id, a.path || ' / ' || c.title
             from modules c join ancestry a on c.parent_id = a.id
           )
           select a.id as "module_id!", a.title as "title!", a.is_folder as "is_folder!",
                  a.status as "status!", a.subject_id, a.path as "path!"
           from ancestry a
           inner join module_labels l on l.module_id = a.id
           where l.kind = $1 and l.value = $2
           order by a.path asc"#,
        kind,
        value,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| LabeledModule {
            module_id: r.module_id,
            title: r.title,
            is_folder: r.is_folder,
            subject_id: r.subject_id,
            status: r.status,
            path: r.path,
        })
        .collect())
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct LabelPair {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ModuleLabelRollup {
    pub module_id: Uuid,
    pub labels: Vec<LabelPair>,
}

#[derive(Debug, serde::Serialize)]
pub struct ListRollupResponse {
    pub items: Vec<ModuleLabelRollup>,
}

// GET /modules/label-rollup?parent_id= — labels roll UP the tree: each
// direct child of `parent_id` reports the deduped union of every label
// in its own subtree, itself included.
//
// This is what lets an outer folder advertise what is inside it. A
// learner opening "Matematika" sees on the "Tahap 3" card that it spans
// SMA-10..12 and feeds UTBK — without opening it and without any Bab
// having to repeat its parent's labels. One query for the whole
// listing, so the browse page stays a single request rather than N+1.
pub async fn rollup_for_children(pool: &PgPool, parent_id: Option<Uuid>) -> Result<ListRollupResponse, AppError> {
    let rows = sqlx::query!(
        r#"with recursive subtree as (
             -- Seed: each direct child is the root of its own subtree,
             -- and carries root_id down through every recursion step.
             select m.id as root_id, m.id as id
             from modules m
             where m.parent_id is not distinct from $1
             union all
             select s.root_id, c.id
             from modules c join subtree s on c.parent_id = s.id
           )
           select distinct s.root_id as "root_id!", l.kind as "kind!", l.value as "value!"
           from subtree s
           -- Migration 0042 — a reference row owns no labels either; its
           -- meaning comes from the library module it points at. Without
           -- this coalesce, a learning path built entirely out of
           -- references reports no labels at all.
           inner join modules m on m.id = s.id
           inner join module_labels l on l.module_id = coalesce(m.source_module_id, m.id)
           order by s.root_id, l.kind, l.value"#,
        parent_id,
    )
    .fetch_all(pool)
    .await?;

    // Rows arrive grouped by root_id (ORDER BY above), so a single pass
    // folds them into one entry per child.
    let mut items: Vec<ModuleLabelRollup> = Vec::new();
    for r in rows {
        match items.last_mut() {
            Some(last) if last.module_id == r.root_id => last.labels.push(LabelPair { kind: r.kind, value: r.value }),
            _ => items.push(ModuleLabelRollup { module_id: r.root_id, labels: vec![LabelPair { kind: r.kind, value: r.value }] }),
        }
    }
    Ok(ListRollupResponse { items })
}

#[derive(Debug, serde::Serialize)]
pub struct LabelSummary {
    pub kind: String,
    pub value: String,
    pub module_count: i64,
}

// GET /module-labels/summary — the coverage map: every label in use and
// how many modules carry it. This is what tells a curriculum agent (or
// a human curator) which parts of the curriculum are thin.
pub async fn summary(pool: &PgPool) -> Result<Vec<LabelSummary>, AppError> {
    let rows = sqlx::query!(
        r#"select kind as "kind!", value as "value!", count(*) as "module_count!"
           from module_labels group by kind, value order by kind asc, value asc"#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| LabelSummary { kind: r.kind, value: r.value, module_count: r.module_count }).collect())
}

#[derive(Debug, serde::Serialize)]
pub struct LabelPreview {
    pub kind: String,
    pub value: String,
    pub topic_count: i64,
    pub subject_count: i64,
    /// A few real topic titles, so the onboarding preview shows actual
    /// curriculum rather than a promise.
    pub sample_titles: Vec<String>,
}

// GET /public/curriculum-preview?labels=ujian:utbk-pm,jenjang:SMA-11
//
// Deliberately PUBLIC and deliberately counts-only: the new onboarding
// flow lets a visitor state a goal and see the real path waiting for
// them BEFORE being asked to sign in. Showing invented numbers there
// would make the first honest moment of the product a lie, so this
// reads the same labels the signed-in browser does — it just returns
// counts and a handful of titles instead of the full tree.
pub async fn preview(pool: &PgPool, kind: &str, value: &str) -> Result<LabelPreview, AppError> {
    let rows = sqlx::query!(
        r#"with recursive ancestry as (
             select m.id, m.parent_id, m.title, m.is_folder, m.title as path
             from modules m where m.parent_id is null
             union all
             select c.id, c.parent_id, c.title, c.is_folder, a.path || ' / ' || c.title
             from modules c join ancestry a on c.parent_id = a.id
           )
           select a.title as "title!", a.path as "path!"
           from ancestry a
           inner join module_labels l on l.module_id = a.id
           where l.kind = $1 and l.value = $2 and a.is_folder = false
           order by a.path asc"#,
        kind,
        value,
    )
    .fetch_all(pool)
    .await?;

    let topic_count = rows.len() as i64;
    let mut subjects = std::collections::HashSet::new();
    for r in &rows {
        // path = "<root> / <subject> / ..." — the subject is segment 2.
        if let Some(subject) = r.path.split(" / ").nth(1) {
            subjects.insert(subject.to_string());
        }
    }
    // Spread the samples across the list rather than taking the first
    // few, which would all come from one chapter.
    let step = (topic_count as usize / 4).max(1);
    let sample_titles = rows.iter().step_by(step).take(4).map(|r| r.title.clone()).collect();

    Ok(LabelPreview {
        kind: kind.to_string(),
        value: value.to_string(),
        topic_count,
        subject_count: subjects.len() as i64,
        sample_titles,
    })
}

// --- Landing page preview (migration 0041) ---

// A real subtree node, at whatever depth the curriculum actually has —
// not flattened to a fixed number of tiers. A leaf topic is a node with
// an empty `children` (its own module has no folder children).
// `topic_count` is always the number of LEAF descendants: 1 for a leaf,
// the sum of its children's counts for a folder, so a caller never has
// to walk the tree just to show "how much is in here".
#[derive(Debug, serde::Serialize, Clone)]
pub struct PathNode {
    pub id: Uuid,
    pub title: String,
    pub is_folder: bool,
    pub topic_count: i64,
    /// Rolled-up over the node's whole subtree, resolved through
    /// reference rows — a learning path made only of references would
    /// otherwise report nothing.
    pub labels: Vec<LabelPair>,
    /// Set when the node has children that were NOT sent, so the client
    /// knows to fetch them instead of treating the node as a leaf.
    pub has_more: bool,
    pub children: Vec<PathNode>,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct LearningPathPreview {
    pub id: Uuid,
    pub title: String,
    pub section_count: i64,
    pub topic_count: i64,
    pub sections: Vec<PathNode>,
}

struct TreeRow {
    id: Uuid,
    parent_id: Option<Uuid>,
    title: String,
    is_folder: bool,
}

// Builds one PathNode from a flat row list, via an id -> row-index map
// (O(1) per lookup rather than scanning `rows` at every recursive
// step) and a parent -> children-indices map for the same reason.
// Landing payload sends the tree only this deep. The full curriculum is
// ~20k nodes; shipping all of it made the marketing page a 1.3 MB
// download before anything was even clicked. Deeper levels arrive from
// `children_of` when the learner actually opens a folder.
const LANDING_DEPTH: i32 = 1;

type LabelSet = std::collections::BTreeSet<(String, String)>;

// Returns the node AND its subtree's label set, so the caller can fold
// child sets into the parent without walking the tree a second time.
// topic_count is always computed over the WHOLE subtree even when the
// node itself is not serialised, so a collapsed folder still reports
// honestly how much is inside.
fn build_node(
    id: Uuid,
    depth: i32,
    by_id: &std::collections::HashMap<Uuid, usize>,
    by_parent: &std::collections::HashMap<Uuid, Vec<usize>>,
    rows: &[TreeRow],
    labels_by_module: &std::collections::HashMap<Uuid, Vec<(String, String)>>,
) -> (PathNode, LabelSet) {
    let row = &rows[by_id[&id]];

    let mut set: LabelSet = labels_by_module
        .get(&id)
        .map(|v| v.iter().cloned().collect())
        .unwrap_or_default();

    let mut children = Vec::new();
    let mut topic_count = 0i64;
    let child_idxs = by_parent.get(&id);
    for &i in child_idxs.map(|v| v.as_slice()).unwrap_or(&[]) {
        let (node, child_set) = build_node(rows[i].id, depth + 1, by_id, by_parent, rows, labels_by_module);
        topic_count += node.topic_count;
        set.extend(child_set);
        if depth < LANDING_DEPTH {
            children.push(node);
        }
    }

    let has_children = child_idxs.map(|v| !v.is_empty()).unwrap_or(false);
    if !row.is_folder {
        topic_count = 1;
    }

    (
        PathNode {
            id: row.id,
            title: row.title.clone(),
            is_folder: row.is_folder,
            topic_count,
            labels: set.iter().map(|(k, v)| LabelPair { kind: k.clone(), value: v.clone() }).collect(),
            has_more: has_children && children.is_empty(),
            children,
        },
        set,
    )
}

// GET /public/learning-paths — deliberately public and deliberately
// eager: it returns the FULL nested tree for every path carrying
// tema=landing-featured in one call, so the landing page's explorer can
// switch between paths and drill through them instantly with no
// per-click fetch. ~100KB across the 8 curated paths today (7 exam/
// curriculum paths plus the full "Semua Mata Pelajaran" library),
// trivial for a landing page.
//
// The tree is built to WHATEVER depth the curriculum actually has —
// earlier this flattened everything past 2 fixed tiers into one bucket
// (a Tahap with 17 Bab dumped 107 topics into a single undifferentiated
// list); a real recursive tree keeps every Bab as its own node instead.
// The landing browser is the most-hit unauthenticated endpoint and its
// answer changes only when the curriculum itself is edited — which is a
// deliberate, infrequent act. Walking eight full path subtrees plus
// every label on each request cost ~3s; cache the assembled preview
// rather than paying that per visitor.
//
// The cache is keyed on a cheap freshness probe rather than a timer.
// A TTL alone was actively wrong here: the payload carries module IDs,
// and after a curriculum edit the stale entry hands out IDs of rows
// that no longer exist, so every folder the visitor opens comes back
// empty until the timer happens to lapse. Probing the tree's row count
// and newest mtime catches edits made through the API *and* straight
// through SQL, and costs a fraction of a millisecond.
type LandingKey = (i64, Option<chrono::DateTime<chrono::Utc>>);
static LANDING_CACHE: std::sync::OnceLock<tokio::sync::RwLock<Option<(LandingKey, Vec<LearningPathPreview>)>>> =
    std::sync::OnceLock::new();

async fn landing_key(pool: &PgPool) -> Result<LandingKey, AppError> {
    let row = sqlx::query!(
        r#"select count(*) as "n!", max(updated_at) as "at" from modules"#
    )
    .fetch_one(pool)
    .await?;
    Ok((row.n, row.at))
}

pub async fn list_landing_paths(pool: &PgPool) -> Result<Vec<LearningPathPreview>, AppError> {
    let cache = LANDING_CACHE.get_or_init(|| tokio::sync::RwLock::new(None));
    let key = landing_key(pool).await?;
    if let Some((cached_key, cached)) = cache.read().await.as_ref() {
        if *cached_key == key {
            return Ok(cached.clone());
        }
    }
    let fresh = build_landing_paths(pool).await?;
    *cache.write().await = Some((key, fresh.clone()));
    Ok(fresh)
}

async fn build_landing_paths(pool: &PgPool) -> Result<Vec<LearningPathPreview>, AppError> {
    let roots = sqlx::query!(
        r#"select m.id, m.title, m.order_index
           from modules m
           inner join module_labels l on l.module_id = m.id
           where l.kind = 'tema' and l.value = 'landing-featured'
           order by m.order_index asc"#,
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(roots.len());
    for root in roots {
        // The path's ENTIRE subtree in one query, ordered so every row's
        // parent already appears earlier — that's what lets build_node
        // recurse without further round-trips. Walking per section
        // instead re-queried overlapping subtrees and made this endpoint
        // take seconds.
        let rows = sqlx::query_as!(
            TreeRow,
            r#"with recursive walk as (
                 select m.id, m.parent_id, m.title, m.is_folder, m.order_index, 0 as depth
                 from modules m where m.id = $1
                 union all
                 select c.id, c.parent_id, c.title, c.is_folder, c.order_index, w.depth + 1
                 from modules c join walk w on c.parent_id = w.id
               )
               select id as "id!", parent_id, title as "title!", is_folder as "is_folder!"
               from walk
               order by depth asc, order_index asc"#,
            root.id,
        )
        .fetch_all(pool)
        .await?;

        let label_rows = sqlx::query!(
            r#"with recursive walk as (
                 select m.id from modules m where m.id = $1
                 union all
                 select c.id from modules c join walk w on c.parent_id = w.id
               )
               select distinct w.id as "id!", l.kind as "kind!", l.value as "value!"
               from walk w
               join modules m on m.id = w.id
               join module_labels l on l.module_id = coalesce(m.source_module_id, m.id)"#,
            root.id,
        )
        .fetch_all(pool)
        .await?;
        let mut labels_by_module: std::collections::HashMap<Uuid, Vec<(String, String)>> = std::collections::HashMap::new();
        for r in label_rows {
            labels_by_module.entry(r.id).or_default().push((r.kind, r.value));
        }

        let mut by_id: std::collections::HashMap<Uuid, usize> = std::collections::HashMap::with_capacity(rows.len());
        let mut by_parent: std::collections::HashMap<Uuid, Vec<usize>> = std::collections::HashMap::new();
        for (i, r) in rows.iter().enumerate() {
            by_id.insert(r.id, i);
            if let Some(pid) = r.parent_id {
                by_parent.entry(pid).or_default().push(i);
            }
        }

        // Depth is seeded at -1 so the path root itself sits above the
        // sections, and sections land at depth 0 like before.
        let (node, _) = build_node(root.id, -1, &by_id, &by_parent, &rows, &labels_by_module);

        out.push(LearningPathPreview {
            id: root.id,
            title: root.title,
            section_count: node.children.len() as i64,
            topic_count: node.topic_count,
            sections: node.children,
        });
    }

    Ok(out)
}

// GET /public/learning-paths/children?parent_id= — satu tingkat isi folder,
// dipakai saat pengunjung membuka sebuah folder di penjelajah materi.
// Payload landing sengaja dangkal (lihat LANDING_DEPTH); ini pasangannya.
pub async fn landing_children(pool: &PgPool, parent_id: Uuid) -> Result<Vec<PathNode>, AppError> {
    let rows = sqlx::query_as!(
        TreeRow,
        r#"with recursive walk as (
             select m.id, m.parent_id, m.title, m.is_folder, m.order_index, 0 as depth
             from modules m where m.parent_id = $1
             union all
             select c.id, c.parent_id, c.title, c.is_folder, c.order_index, w.depth + 1
             from modules c join walk w on c.parent_id = w.id
           )
           select id as "id!", parent_id, title as "title!", is_folder as "is_folder!"
           from walk order by depth asc, order_index asc"#,
        parent_id,
    )
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(vec![]);
    }

    let label_rows = sqlx::query!(
        r#"with recursive walk as (
             select m.id from modules m where m.parent_id = $1
             union all
             select c.id from modules c join walk w on c.parent_id = w.id
           )
           select distinct w.id as "id!", l.kind as "kind!", l.value as "value!"
           from walk w
           join modules m on m.id = w.id
           join module_labels l on l.module_id = coalesce(m.source_module_id, m.id)"#,
        parent_id,
    )
    .fetch_all(pool)
    .await?;
    let mut labels_by_module: std::collections::HashMap<Uuid, Vec<(String, String)>> = std::collections::HashMap::new();
    for r in label_rows {
        labels_by_module.entry(r.id).or_default().push((r.kind, r.value));
    }

    let mut by_id: std::collections::HashMap<Uuid, usize> = std::collections::HashMap::with_capacity(rows.len());
    let mut by_parent: std::collections::HashMap<Uuid, Vec<usize>> = std::collections::HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        by_id.insert(r.id, i);
        if let Some(pid) = r.parent_id {
            by_parent.entry(pid).or_default().push(i);
        }
    }

    // Only the direct children are returned; build_node still walks the
    // whole subtree so each one reports an honest topic_count.
    let mut out = Vec::new();
    for r in rows.iter().filter(|r| r.parent_id == Some(parent_id)) {
        let (node, _) = build_node(r.id, LANDING_DEPTH, &by_id, &by_parent, &rows, &labels_by_module);
        out.push(node);
    }
    Ok(out)
}
