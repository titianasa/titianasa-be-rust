use std::collections::{HashMap, HashSet};

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};

// Learning paths are ASSEMBLED, never authored. Every row inside one is a
// reference (migration 0042) into the master library; the path itself owns
// no material. What each section should contain is declared once, as a spec
// stored in the path root's `metadata.path_spec`, and this service brings
// the tree in line with that declaration.
//
// The point is that the curriculum keeps moving: topics get added to a
// subject, a label gets corrected, an ordering changes. Rebuilding a path
// by deleting and recreating it would answer that — but it churns every
// row id, and ids are what learner progress, bookmarks and cached landing
// payloads point at. So `sync` reconciles instead: it keeps rows whose
// identity is unchanged, inserts only what is genuinely new, removes only
// what no longer belongs, and rewrites order_index. Running it twice in a
// row is a no-op.

// ---------------------------------------------------------------- the spec

fn default_group_by() -> String {
    "mapel".to_string()
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SectionSpec {
    pub title: String,
    /// A grouping tier. When present the node holds sub-sections rather
    /// than material of its own ("SD / MI — Kelas 1-6" over its classes).
    #[serde(default)]
    pub children: Vec<SectionSpec>,
    /// Library topics carrying ALL of these `kind:value` labels.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Everything under one library branch, for material that has no label
    /// axis of its own (linguistics, say, which is not a CEFR level).
    #[serde(default)]
    pub under_path: Option<String>,
    /// Restrict to a single library subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// Whitelist / blacklist of library subjects.
    #[serde(default)]
    pub subjects: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
    /// How the resolved topics are foldered: by subject ("mapel") or by the
    /// library chapter they come from ("bab").
    #[serde(default = "default_group_by")]
    pub group_by: String,
    /// Display grouping: several subjects shown under one name, for levels
    /// that teach them as a single subject (Sejarah/Ekonomi -> IPS at SMP).
    #[serde(default)]
    pub alias: HashMap<String, String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct PathSpec {
    pub sections: Vec<SectionSpec>,
}

// ------------------------------------------------------------ what we want

// The tree the spec asks for, resolved against the library but not yet
// written. Built entirely before anything is touched, so a spec that
// resolves to nothing can be rejected before it empties a live path.
#[derive(Debug)]
enum Desired {
    Folder { title: String, children: Vec<Desired> },
    Reference { title: String, source_id: Uuid },
}

impl Desired {
    /// Identity used to match an existing row. A reference is identified by
    /// what it points at, not by its title, so renaming a library topic
    /// moves the title across without replacing the row.
    fn key(&self) -> String {
        match self {
            Desired::Folder { title, .. } => format!("f:{title}"),
            Desired::Reference { source_id, .. } => format!("r:{source_id}"),
        }
    }
    fn title(&self) -> &str {
        match self {
            Desired::Folder { title, .. } | Desired::Reference { title, .. } => title,
        }
    }
    fn leaf_count(&self) -> i64 {
        match self {
            Desired::Reference { .. } => 1,
            Desired::Folder { children, .. } => children.iter().map(|c| c.leaf_count()).sum(),
        }
    }
}

struct LibTopic {
    id: Uuid,
    title: String,
    path: String,
}

impl LibTopic {
    fn subject(&self) -> &str {
        self.path.split(" / ").nth(1).unwrap_or("")
    }
    fn chapter(&self) -> &str {
        let seg: Vec<&str> = self.path.split(" / ").collect();
        if seg.len() >= 5 { seg[3] } else { seg.get(2).copied().unwrap_or("Umum") }
    }
}

const LIBRARY_ROOT: &str = "Semua Mata Pelajaran";

// Topics carrying every requested label, or everything under one branch.
// Only real library material: folders and reference rows are excluded, so a
// path can never end up referencing another path's rows.
async fn resolve_topics(pool: &PgPool, spec: &SectionSpec) -> Result<Vec<LibTopic>, AppError> {
    let prefix = format!("{LIBRARY_ROOT} / ");
    let rows = if let Some(branch) = &spec.under_path {
        sqlx::query!(
            r#"with recursive anc as (
                 select m.id, m.parent_id, m.is_folder, m.source_module_id, m.title, m.title as path
                 from modules m where m.parent_id is null
                 union all
                 select c.id, c.parent_id, c.is_folder, c.source_module_id, c.title, a.path || ' / ' || c.title
                 from modules c join anc a on c.parent_id = a.id
               )
               select a.id as "id!", a.title as "title!", a.path as "path!"
               from anc a
               where not a.is_folder and a.source_module_id is null and a.path like $1 || '%'
               order by a.path asc"#,
            branch,
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| LibTopic { id: r.id, title: r.title, path: r.path })
        .collect::<Vec<_>>()
    } else {
        if spec.labels.is_empty() {
            return Ok(vec![]);
        }
        let pairs: Vec<String> = spec.labels.clone();
        sqlx::query!(
            r#"with recursive anc as (
                 select m.id, m.parent_id, m.is_folder, m.source_module_id, m.title, m.title as path
                 from modules m where m.parent_id is null
                 union all
                 select c.id, c.parent_id, c.is_folder, c.source_module_id, c.title, a.path || ' / ' || c.title
                 from modules c join anc a on c.parent_id = a.id
               )
               select a.id as "id!", a.title as "title!", a.path as "path!"
               from anc a
               where not a.is_folder and a.source_module_id is null and a.path like $2
                 and (select count(distinct l.kind || ':' || l.value)
                        from module_labels l
                       where l.module_id = a.id and l.kind || ':' || l.value = any($1)) = array_length($1, 1)
               order by a.path asc"#,
            &pairs,
            format!("{prefix}%"),
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| LibTopic { id: r.id, title: r.title, path: r.path })
        .collect::<Vec<_>>()
    };

    Ok(rows
        .into_iter()
        .filter(|t| {
            let s = t.subject();
            if let Some(only) = &spec.subject {
                if s != only {
                    return false;
                }
            }
            if let Some(list) = &spec.subjects {
                if !list.iter().any(|x| x == s) {
                    return false;
                }
            }
            if let Some(list) = &spec.exclude {
                if list.iter().any(|x| x == s) {
                    return false;
                }
            }
            true
        })
        .collect())
}

// One section becomes: group folders (by subject or by library chapter),
// each holding its topics as references, both in library order.
fn group_topics(spec: &SectionSpec, topics: Vec<LibTopic>) -> Vec<Desired> {
    let mut order: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, Vec<LibTopic>> = HashMap::new();
    for t in topics {
        let raw = if spec.group_by == "mapel" { t.subject() } else { t.chapter() };
        let key = spec.alias.get(raw).cloned().unwrap_or_else(|| raw.to_string());
        if !by_key.contains_key(&key) {
            order.push(key.clone());
        }
        by_key.entry(key).or_default().push(t);
    }
    order.sort();
    order
        .into_iter()
        .map(|k| {
            let list = by_key.remove(&k).unwrap_or_default();
            Desired::Folder {
                title: k,
                children: list
                    .into_iter()
                    .map(|t| Desired::Reference { title: t.title, source_id: t.id })
                    .collect(),
            }
        })
        .collect()
}

async fn build_desired(pool: &PgPool, specs: &[SectionSpec]) -> Result<Vec<Desired>, AppError> {
    let mut out = Vec::new();
    for spec in specs {
        let node = if spec.children.is_empty() {
            let topics = resolve_topics(pool, spec).await?;
            if topics.is_empty() {
                // A section with nothing behind it is a dead end in the UI,
                // so it is left out rather than published empty.
                continue;
            }
            Desired::Folder { title: spec.title.clone(), children: group_topics(spec, topics) }
        } else {
            let children = Box::pin(build_desired(pool, &spec.children)).await?;
            if children.is_empty() {
                continue;
            }
            Desired::Folder { title: spec.title.clone(), children }
        };
        out.push(node);
    }
    Ok(out)
}

// ---------------------------------------------------------------- the sync

#[derive(Debug, Default, serde::Serialize)]
pub struct SyncReport {
    pub path: String,
    pub inserted: i64,
    pub removed: i64,
    pub reordered: i64,
    pub kept: i64,
    pub references: i64,
}

struct ExistingRow {
    id: Uuid,
    title: String,
    order_index: i32,
    source_module_id: Option<Uuid>,
}

// Removing a folder means removing what is under it; modules.parent_id has
// no cascade, so the subtree is collected first and deleted in one
// statement (RI is checked at statement end, which lets parent and child
// go together).
async fn delete_subtree(pool: &PgPool, ids: &[Uuid]) -> Result<i64, AppError> {
    if ids.is_empty() {
        return Ok(0);
    }
    // Defensive symmetry with module.rs::delete's guard, even though a
    // sync-driven removal here is provably scoped to folders and
    // reference rows (validate()'s "learning path owns no material"
    // invariant means nothing sync ever deletes can carry its own
    // items) — cheap to check, and it stays true only as long as that
    // invariant does.
    let attempt_count = sqlx::query_scalar!(
        r#"select count(*) as "n!" from attempts a
           where a.item_id in (select i.id from module_items i where i.module_id = any($1))"#,
        ids,
    )
    .fetch_one(pool)
    .await?;
    if attempt_count > 0 {
        return Err(AppError::UnprocessableEntity(
            "path_node_has_attempts",
            format!("refusing to remove {attempt_count} row(s) with learner attempts during sync"),
        ));
    }

    let n = sqlx::query!(
        r#"with recursive doomed as (
             select id from modules where id = any($1)
             union all
             select m.id from modules m join doomed d on m.parent_id = d.id
           ),
           all_ids as (select distinct id from doomed),
           del_blocks as (
             delete from content_blocks
              where item_id in (select i.id from module_items i where i.module_id in (select id from all_ids))
           ),
           del_refs as (
             delete from modules where source_module_id in (select id from all_ids)
           )
           delete from modules where id in (select id from all_ids)"#,
        ids,
    )
    .execute(pool)
    .await?;
    Ok(n.rows_affected() as i64)
}

async fn reconcile(
    pool: &PgPool,
    parent_id: Uuid,
    desired: &[Desired],
    report: &mut SyncReport,
) -> Result<(), AppError> {
    let existing: Vec<ExistingRow> = sqlx::query_as!(
        ExistingRow,
        r#"select id as "id!", title as "title!", order_index as "order_index!", source_module_id
           from modules where parent_id = $1"#,
        parent_id,
    )
    .fetch_all(pool)
    .await?;

    let mut by_key: HashMap<String, &ExistingRow> = HashMap::new();
    for row in &existing {
        let key = match row.source_module_id {
            Some(src) => format!("r:{src}"),
            None => format!("f:{}", row.title),
        };
        // A duplicate key can only come from an older, non-reconciling
        // build; keep the first and let the rest fall through to removal.
        by_key.entry(key).or_insert(row);
    }

    let mut kept_ids: HashSet<Uuid> = HashSet::new();

    for (i, want) in desired.iter().enumerate() {
        let idx = i as i32;
        let key = want.key();
        let id = match by_key.get(&key) {
            Some(row) => {
                kept_ids.insert(row.id);
                report.kept += 1;
                if row.order_index != idx || row.title != want.title() {
                    sqlx::query!(
                        r#"update modules set title = $2, order_index = $3, updated_at = now() where id = $1"#,
                        row.id,
                        want.title(),
                        idx,
                    )
                    .execute(pool)
                    .await?;
                    report.reordered += 1;
                }
                row.id
            }
            None => {
                let new_id = match want {
                    Desired::Folder { title, .. } => {
                        sqlx::query_scalar!(
                            r#"insert into modules (parent_id, is_folder, title, status, order_index, generated_by)
                               values ($1, true, $2, 'draft', $3, 'ai') returning id"#,
                            parent_id,
                            title,
                            idx,
                        )
                        .fetch_one(pool)
                        .await?
                    }
                    Desired::Reference { title, source_id } => {
                        // The reference carries the source's subject so that
                        // per-subject filters keep working on it.
                        sqlx::query_scalar!(
                            r#"insert into modules (parent_id, is_folder, subject_id, title, status,
                                                    order_index, generated_by, source_module_id)
                               select $1, false, s.subject_id, $2, 'draft', $3, 'ai', s.id
                                 from modules s where s.id = $4
                               returning id"#,
                            parent_id,
                            title,
                            idx,
                            source_id,
                        )
                        .fetch_one(pool)
                        .await?
                    }
                };
                report.inserted += 1;
                new_id
            }
        };
        if let Desired::Reference { .. } = want {
            report.references += 1;
        }
        if let Desired::Folder { children, .. } = want {
            Box::pin(reconcile(pool, id, children, report)).await?;
        }
    }

    let stale: Vec<Uuid> = existing.iter().map(|r| r.id).filter(|id| !kept_ids.contains(id)).collect();
    report.removed += delete_subtree(pool, &stale).await?;
    Ok(())
}

pub async fn sync_path(pool: &PgPool, ctx: &AuthContext, path_id: Uuid) -> Result<SyncReport, AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;

    let root = sqlx::query!(
        r#"select title as "title!", metadata from modules where id = $1 and parent_id is null"#,
        path_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("learning_path_not_found"))?;

    let spec_json = root
        .metadata
        .as_ref()
        .and_then(|m| m.get("path_spec").cloned())
        .ok_or_else(|| {
            AppError::UnprocessableEntity(
                "path_spec_missing",
                "this module has no metadata.path_spec to assemble from".to_string(),
            )
        })?;
    let spec: PathSpec = serde_json::from_value(spec_json).map_err(|e| {
        AppError::UnprocessableEntity("path_spec_invalid", format!("metadata.path_spec is malformed: {e}"))
    })?;

    let desired = build_desired(pool, &spec.sections).await?;
    let want_leaves: i64 = desired.iter().map(|d| d.leaf_count()).sum();
    if want_leaves == 0 {
        // Almost always a mistyped label rather than a real intention to
        // empty the path, and emptying it would delete every reference.
        return Err(AppError::UnprocessableEntity(
            "path_spec_resolves_empty",
            "the spec matched no library topics; refusing to empty the path".to_string(),
        ));
    }

    let mut report = SyncReport { path: root.title, ..Default::default() };
    reconcile(pool, path_id, &desired, &mut report).await?;
    Ok(report)
}

pub async fn sync_all(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<SyncReport>, AppError> {
    let roots = sqlx::query!(
        r#"select id as "id!" from modules
           where parent_id is null and metadata ? 'path_spec'
           order by order_index asc"#,
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for r in roots {
        out.push(sync_path(pool, ctx, r.id).await?);
    }
    Ok(out)
}

// ---------------------------------------------------------- the guard rails

#[derive(Debug, serde::Serialize)]
pub struct Check {
    pub name: String,
    pub count: i64,
    pub ok: bool,
    /// A few offending rows, so a failure is actionable without a query.
    pub sample: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ValidationReport {
    pub ok: bool,
    pub checks: Vec<Check>,
}

// The invariants that make the library trustworthy as a single source of
// truth. Ran by hand until now; running them from code is what keeps a
// future edit — a new topic, a relabelled chapter — from quietly breaking
// something nobody re-checks.
pub async fn validate(pool: &PgPool) -> Result<ValidationReport, AppError> {
    struct Q {
        name: &'static str,
        sql: &'static str,
    }
    // Each query returns rows that should not exist; empty means healthy.
    let queries = [
        Q { name: "topik perpustakaan tanpa jenjang tunggal", sql:
            r#"with recursive anc as (select id,parent_id,is_folder,source_module_id,title as path from modules where parent_id is null
                 union all select c.id,c.parent_id,c.is_folder,c.source_module_id,a.path||' / '||c.title from modules c join anc a on c.parent_id=a.id)
               select a.path from anc a where a.path like 'Semua Mata Pelajaran / %' and not a.is_folder and a.source_module_id is null
                 and (select count(*) from module_labels l where l.module_id=a.id and l.kind='jenjang') <> 1"# },
        Q { name: "topik dengan lebih dari satu level CEFR", sql:
            r#"with recursive anc as (select id,parent_id,is_folder,source_module_id,title as path from modules where parent_id is null
                 union all select c.id,c.parent_id,c.is_folder,c.source_module_id,a.path||' / '||c.title from modules c join anc a on c.parent_id=a.id)
               select a.path from anc a where a.path like 'Semua Mata Pelajaran / %' and not a.is_folder and a.source_module_id is null
                 and (select count(*) from module_labels l where l.module_id=a.id and l.kind='cefr') > 1"# },
        Q { name: "learning path memiliki materi sendiri (bukan referensi)", sql:
            r#"with recursive anc as (select id,parent_id,is_folder,source_module_id,title as path from modules where parent_id is null
                 union all select c.id,c.parent_id,c.is_folder,c.source_module_id,a.path||' / '||c.title from modules c join anc a on c.parent_id=a.id)
               select a.path from anc a where a.path not like 'Semua Mata Pelajaran%' and not a.is_folder and a.source_module_id is null"# },
        Q { name: "referensi menggantung (sumber sudah tidak ada)", sql:
            r#"select m.title from modules m where m.source_module_id is not null
                 and not exists (select 1 from modules s where s.id = m.source_module_id)"# },
        Q { name: "referensi berantai (menunjuk referensi lain)", sql:
            r#"select a.title from modules a join modules b on a.source_module_id = b.id where b.source_module_id is not null"# },
        Q { name: "referensi ke luar perpustakaan induk", sql:
            r#"with recursive anc as (select id,parent_id,title as path from modules where parent_id is null
                 union all select c.id,c.parent_id,a.path||' / '||c.title from modules c join anc a on c.parent_id=a.id)
               select a.path from modules m join anc a on a.id=m.source_module_id where a.path not like 'Semua Mata Pelajaran / %'"# },
        Q { name: "folder kosong", sql:
            r#"with recursive anc as (select id,parent_id,is_folder,title as path from modules where parent_id is null
                 union all select c.id,c.parent_id,c.is_folder,a.path||' / '||c.title from modules c join anc a on c.parent_id=a.id)
               select a.path from anc a where a.is_folder and not exists (select 1 from modules m where m.parent_id=a.id)"# },
        Q { name: "topik yang sama muncul dua kali dalam satu folder", sql:
            r#"select coalesce(p.title,'(akar)') from modules m join modules p on p.id=m.parent_id
               where m.source_module_id is not null
               group by p.title, m.parent_id, m.source_module_id having count(*) > 1"# },
        Q { name: "topik perpustakaan tanpa 2 artikel + 2 kuis", sql:
            r#"with recursive anc as (select id,parent_id,is_folder,source_module_id,title as path from modules where parent_id is null
                 union all select c.id,c.parent_id,c.is_folder,c.source_module_id,a.path||' / '||c.title from modules c join anc a on c.parent_id=a.id)
               select a.path from anc a where a.path like 'Semua Mata Pelajaran / %' and not a.is_folder and a.source_module_id is null
                 and ((select count(*) from module_items i where i.module_id=a.id and i.content_type='article') <> 2
                   or (select count(*) from module_items i where i.module_id=a.id and i.content_type='quiz') <> 2)"# },
        Q { name: "topik perpustakaan tidak terjangkau learning path mana pun", sql:
            r#"with recursive anc as (select id,parent_id,is_folder,source_module_id,title as path from modules where parent_id is null
                 union all select c.id,c.parent_id,c.is_folder,c.source_module_id,a.path||' / '||c.title from modules c join anc a on c.parent_id=a.id)
               select a.path from anc a where a.path like 'Semua Mata Pelajaran / %' and not a.is_folder and a.source_module_id is null
                 and not exists (select 1 from modules r where r.source_module_id = a.id)"# },
        Q { name: "modul yatim (induk sudah tidak ada)", sql:
            r#"select m.title from modules m where m.parent_id is not null
                 and not exists (select 1 from modules p where p.id = m.parent_id)"# },
    ];

    let mut checks = Vec::new();
    let mut ok_all = true;
    for q in queries {
        let rows: Vec<String> = sqlx::query_scalar(q.sql).fetch_all(pool).await?;
        let ok = rows.is_empty();
        ok_all &= ok;
        checks.push(Check {
            name: q.name.to_string(),
            count: rows.len() as i64,
            ok,
            sample: rows.into_iter().take(5).collect(),
        });
    }
    Ok(ValidationReport { ok: ok_all, checks })
}
