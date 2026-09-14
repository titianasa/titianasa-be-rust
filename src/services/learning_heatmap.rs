// Heatmaps of how well answers go, sliced by where questions sit in the
// curriculum and by what kind of question they are (migrations/0058).
//
// One implementation, three scopes: the whole platform (Admin Pusat,
// read from the daily rollup — ADR-0014 never lets a dashboard scan a
// transaction table on page load), one learner (their own Progres), and
// a class (its teacher). The drill-down is the same everywhere: root
// folders → subject → Tahap/domain folders → topik → bab, walking each
// fact's folder path, so a subject with one folder level fewer needs no
// special case. A level with a single child is skipped automatically —
// nobody wants to click "Semua Mata Pelajaran" to reach "Matematika".

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};

const BLOOM: [&str; 6] = ["c1", "c2", "c3", "c4", "c5", "c6"];
const DIFFICULTY: [&str; 3] = ["mudah", "sedang", "sulit"];
const UNLABELLED: &str = "tanpa_label";
/// "Soal tersulit" needs enough answers for p to mean anything.
const MIN_GRADED_PLATFORM: i64 = 5;
const MIN_GRADED_CLASS: i64 = 3;

#[derive(Debug, Deserialize)]
pub struct HeatmapQuery {
    pub parent_id: Option<Uuid>,
    /// "bloom" (default) or "difficulty".
    pub axis: Option<String>,
    /// 1..=365, default 90. Ignored when `from`/`to` are given.
    pub days: Option<i64>,
    /// Platform scope: an explicit WIB date range (Admin Pusat's "Rentang").
    pub from: Option<chrono::NaiveDate>,
    pub to: Option<chrono::NaiveDate>,
    /// Class scope only: one member instead of the whole class.
    pub student_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Folder,
    Module,
    Bab,
}

#[derive(Debug, Serialize)]
pub struct HeatCell {
    pub key: String,
    pub graded: i64,
    pub correct: i64,
}

#[derive(Debug, Serialize)]
pub struct HeatRow {
    pub id: Uuid,
    pub title: String,
    pub kind: NodeKind,
    pub graded: i64,
    pub correct: i64,
    pub cells: Vec<HeatCell>,
}

#[derive(Debug, Serialize)]
pub struct Crumb {
    pub id: Uuid,
    pub title: String,
    pub kind: NodeKind,
}

#[derive(Debug, Serialize)]
pub struct HardQuestion {
    pub question_uid: Uuid,
    pub content_item_id: Uuid,
    pub summary: Option<String>,
    pub bab_title: Option<String>,
    pub difficulty: Option<String>,
    pub bloom: Option<String>,
    pub graded: i64,
    pub correct: i64,
    /// Labelled "mudah" but most people get it wrong, or "sulit" but
    /// nearly everyone gets it right (ADR-0013 §2.2).
    pub label_mismatch: bool,
}

#[derive(Debug, Serialize)]
pub struct HeatmapResponse {
    pub axis: String,
    pub days: i64,
    pub columns: Vec<String>,
    /// From the top down to the level `rows` are the children of.
    pub breadcrumb: Vec<Crumb>,
    pub rows: Vec<HeatRow>,
    pub total_graded: i64,
    pub total_correct: i64,
    pub hardest: Vec<HardQuestion>,
}

/// The finest grain every level is summed from.
#[derive(Debug, Clone)]
pub struct Grain {
    pub folder_path: Vec<Uuid>,
    pub bab_id: Option<Uuid>,
    pub difficulty: Option<String>,
    pub bloom: Option<String>,
    pub graded: i64,
    pub correct: i64,
}

enum Scope {
    Platform,
    Users(Vec<Uuid>),
}

/// Which child of `parent` a grain belongs to. `None` when the grain is
/// outside `parent`, or `parent` is already a bab (the leaf level).
fn child_of(grain: &Grain, parent: Option<Uuid>) -> Option<(Uuid, NodeKind)> {
    let path = &grain.folder_path;
    let kind_at = |i: usize| if i + 1 == path.len() { NodeKind::Module } else { NodeKind::Folder };
    match parent {
        None => path.first().map(|id| (*id, kind_at(0))),
        Some(p) => match path.iter().position(|id| *id == p) {
            Some(pos) if pos + 1 < path.len() => Some((path[pos + 1], kind_at(pos + 1))),
            // Content placed straight under a topik, outside any bab, still counts.
            Some(_) => Some((grain.bab_id.unwrap_or(Uuid::nil()), NodeKind::Bab)),
            None => None,
        },
    }
}

/// Resolves the level to show: starting at `parent`, keeps descending
/// while there is exactly one non-bab child. Returns the effective
/// parent and the nodes skipped on the way.
fn settle(grains: &[Grain], mut parent: Option<Uuid>) -> (Option<Uuid>, Vec<(Uuid, NodeKind)>) {
    let mut skipped = Vec::new();
    for _ in 0..10 {
        let mut children: Vec<(Uuid, NodeKind)> = grains.iter().filter_map(|g| child_of(g, parent)).collect();
        children.sort();
        children.dedup();
        match children.as_slice() {
            [(only, kind)] if *kind != NodeKind::Bab => {
                skipped.push((*only, *kind));
                parent = Some(*only);
            }
            _ => break,
        }
    }
    (parent, skipped)
}

/// The WIB days a platform query covers.
fn platform_days(days: i64, range: Option<(chrono::NaiveDate, chrono::NaiveDate)>) -> (chrono::NaiveDate, chrono::NaiveDate) {
    range.unwrap_or_else(|| {
        let today = crate::services::metrics_rollup::wib_today();
        (today - chrono::Duration::days(days - 1), today)
    })
}

async fn load_grains(pool: &PgPool, scope: &Scope, days: i64, range: Option<(chrono::NaiveDate, chrono::NaiveDate)>, parent: Option<Uuid>) -> Result<Vec<Grain>, AppError> {
    Ok(match scope {
        Scope::Platform => {
            let (from, to) = platform_days(days, range);
            sqlx::query!(
                r#"select folder_path, bab_id, difficulty, bloom, sum(graded)::bigint as "graded!", sum(correct)::bigint as "correct!"
                   from metrics_daily_question
                   where day between $1 and $3 and ($2::uuid is null or $2 = any(folder_path) or bab_id = $2)
                   group by folder_path, bab_id, difficulty, bloom"#,
                from,
                parent,
                to,
            )
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| Grain { folder_path: r.folder_path, bab_id: r.bab_id, difficulty: r.difficulty, bloom: r.bloom, graded: r.graded, correct: r.correct })
            .collect()
        }
        Scope::Users(ids) => sqlx::query!(
            r#"select folder_path, bab_id, difficulty, bloom,
                      (count(*) filter (where correct is not null))::bigint as "graded!", (count(*) filter (where correct))::bigint as "correct!"
               from question_answer_facts
               where user_id = any($1) and answered_at >= now() - make_interval(days => $2::int)
                 and ($3::uuid is null or $3 = any(folder_path) or bab_id = $3)
               group by folder_path, bab_id, difficulty, bloom"#,
            ids,
            days as i32,
            parent,
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| Grain { folder_path: r.folder_path, bab_id: r.bab_id, difficulty: r.difficulty, bloom: r.bloom, graded: r.graded, correct: r.correct })
        .collect(),
    })
}

async fn titles(pool: &PgPool, ids: &[Uuid]) -> Result<HashMap<Uuid, String>, AppError> {
    let mut out: HashMap<Uuid, String> = sqlx::query!(r#"select id, title from modules where id = any($1)"#, ids).fetch_all(pool).await?.into_iter().map(|r| (r.id, r.title)).collect();
    for r in sqlx::query!(r#"select id, title from module_items where id = any($1)"#, ids).fetch_all(pool).await? {
        out.entry(r.id).or_insert(r.title);
    }
    Ok(out)
}

async fn hardest(pool: &PgPool, scope: &Scope, days: i64, range: Option<(chrono::NaiveDate, chrono::NaiveDate)>, parent: Option<Uuid>) -> Result<Vec<HardQuestion>, AppError> {
    struct Row {
        question_uid: Uuid,
        content_item_id: Uuid,
        bab_id: Option<Uuid>,
        difficulty: Option<String>,
        bloom: Option<String>,
        graded: i64,
        correct: i64,
    }
    let rows: Vec<Row> = match scope {
        Scope::Platform => {
            let (from, to) = platform_days(days, range);
            sqlx::query!(
                r#"select question_uid, (array_agg(content_item_id order by day desc))[1] as "content_item_id!", (array_agg(bab_id order by day desc))[1] as bab_id,
                          (array_agg(difficulty order by day desc))[1] as difficulty, (array_agg(bloom order by day desc))[1] as bloom,
                          sum(graded)::bigint as "graded!", sum(correct)::bigint as "correct!"
                   from metrics_daily_question
                   where day between $1 and $4 and ($2::uuid is null or $2 = any(folder_path) or bab_id = $2)
                   group by question_uid
                   having sum(graded) >= $3
                   order by sum(correct)::float / sum(graded) asc, sum(graded) desc
                   limit 10"#,
                from,
                parent,
                MIN_GRADED_PLATFORM,
                to,
            )
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| Row { question_uid: r.question_uid, content_item_id: r.content_item_id, bab_id: r.bab_id, difficulty: r.difficulty, bloom: r.bloom, graded: r.graded, correct: r.correct })
            .collect()
        }
        Scope::Users(ids) => sqlx::query!(
            r#"select question_uid, (array_agg(content_item_id order by answered_at desc))[1] as "content_item_id!", (array_agg(bab_id order by answered_at desc))[1] as bab_id,
                      (array_agg(difficulty order by answered_at desc))[1] as difficulty, (array_agg(bloom order by answered_at desc))[1] as bloom,
                      (count(*) filter (where correct is not null))::bigint as "graded!", (count(*) filter (where correct))::bigint as "correct!"
               from question_answer_facts
               where user_id = any($1) and answered_at >= now() - make_interval(days => $2::int)
                 and ($3::uuid is null or $3 = any(folder_path) or bab_id = $3)
               group by question_uid
               having count(*) filter (where correct is not null) >= $4
               order by (count(*) filter (where correct))::float / count(*) filter (where correct is not null) asc
               limit 10"#,
            ids,
            days as i32,
            parent,
            MIN_GRADED_CLASS,
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| Row { question_uid: r.question_uid, content_item_id: r.content_item_id, bab_id: r.bab_id, difficulty: r.difficulty, bloom: r.bloom, graded: r.graded, correct: r.correct })
        .collect(),
    };

    let bab_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.bab_id).collect();
    let bab_titles = titles(pool, &bab_ids).await?;
    let mut out = Vec::new();
    for r in rows {
        let p = r.correct as f64 / r.graded.max(1) as f64;
        out.push(HardQuestion {
            summary: crate::services::content_report::question_summary(pool, r.content_item_id, &r.question_uid.to_string()).await?,
            bab_title: r.bab_id.and_then(|b| bab_titles.get(&b).cloned()),
            label_mismatch: (r.difficulty.as_deref() == Some("mudah") && p < 0.30) || (r.difficulty.as_deref() == Some("sulit") && p > 0.85),
            question_uid: r.question_uid,
            content_item_id: r.content_item_id,
            difficulty: r.difficulty,
            bloom: r.bloom,
            graded: r.graded,
            correct: r.correct,
        });
    }
    Ok(out)
}

async fn build(pool: &PgPool, scope: Scope, query: HeatmapQuery, with_hardest: bool) -> Result<HeatmapResponse, AppError> {
    let axis = if query.axis.as_deref() == Some("difficulty") { "difficulty" } else { "bloom" };
    let days = query.days.unwrap_or(90).clamp(1, 365);
    let range = match (query.from, query.to) {
        (Some(from), Some(to)) if from <= to => Some((from, to)),
        _ => None,
    };
    let grains = load_grains(pool, &scope, days, range, query.parent_id).await?;
    let (parent, skipped) = settle(&grains, query.parent_id);

    // Breadcrumb: the path above the explicit parent (from any grain
    // under it), then whatever `settle` skipped past.
    let mut crumb_ids: Vec<(Uuid, NodeKind)> = Vec::new();
    if let Some(explicit) = query.parent_id {
        if let Some(g) = grains.iter().find(|g| g.folder_path.contains(&explicit)) {
            let pos = g.folder_path.iter().position(|id| *id == explicit).unwrap_or(0);
            for (i, id) in g.folder_path[..=pos].iter().enumerate() {
                crumb_ids.push((*id, if i + 1 == g.folder_path.len() { NodeKind::Module } else { NodeKind::Folder }));
            }
        } else if let Some(g) = grains.iter().find(|g| g.bab_id == Some(explicit)) {
            for (i, id) in g.folder_path.iter().enumerate() {
                crumb_ids.push((*id, if i + 1 == g.folder_path.len() { NodeKind::Module } else { NodeKind::Folder }));
            }
            crumb_ids.push((explicit, NodeKind::Bab));
        }
    }
    crumb_ids.extend(skipped);

    let mut rows_acc: BTreeMap<(Uuid, NodeKind), BTreeMap<String, (i64, i64)>> = BTreeMap::new();
    let (mut total_graded, mut total_correct) = (0, 0);
    for g in &grains {
        let Some(child) = child_of(g, parent) else { continue };
        let column = match axis {
            "difficulty" => g.difficulty.clone(),
            _ => g.bloom.clone(),
        }
        .unwrap_or_else(|| UNLABELLED.to_string());
        let cell = rows_acc.entry(child).or_default().entry(column).or_insert((0, 0));
        cell.0 += g.graded;
        cell.1 += g.correct;
        total_graded += g.graded;
        total_correct += g.correct;
    }

    let mut columns: Vec<String> = if axis == "difficulty" { DIFFICULTY.iter().map(|s| s.to_string()).collect() } else { BLOOM.iter().map(|s| s.to_string()).collect() };
    if rows_acc.values().any(|cells| cells.contains_key(UNLABELLED)) {
        columns.push(UNLABELLED.to_string());
    }

    let mut ids: Vec<Uuid> = rows_acc.keys().map(|(id, _)| *id).collect();
    ids.extend(crumb_ids.iter().map(|(id, _)| *id));
    let names = titles(pool, &ids).await?;

    let mut rows: Vec<HeatRow> = rows_acc
        .into_iter()
        .map(|((id, kind), cells)| {
            let graded = cells.values().map(|c| c.0).sum();
            let correct = cells.values().map(|c| c.1).sum();
            HeatRow {
                id,
                title: names.get(&id).cloned().unwrap_or_else(|| if id.is_nil() { "Tanpa bab".to_string() } else { "(tanpa judul)".to_string() }),
                kind,
                graded,
                correct,
                cells: columns.iter().map(|k| cells.get(k).map(|c| HeatCell { key: k.clone(), graded: c.0, correct: c.1 }).unwrap_or(HeatCell { key: k.clone(), graded: 0, correct: 0 })).collect(),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.title.cmp(&b.title));

    Ok(HeatmapResponse {
        axis: axis.to_string(),
        days,
        columns,
        breadcrumb: crumb_ids.into_iter().map(|(id, kind)| Crumb { title: names.get(&id).cloned().unwrap_or_default(), id, kind }).collect(),
        rows,
        total_graded,
        total_correct,
        hardest: if with_hardest { hardest(pool, &scope, days, range, parent).await? } else { Vec::new() },
    })
}

// GET /me/learning-heatmap
pub async fn for_me(pool: &PgPool, ctx: &AuthContext, query: HeatmapQuery) -> Result<HeatmapResponse, AppError> {
    build(pool, Scope::Users(vec![ctx.user_id]), query, false).await
}

// GET /admin/learning/heatmap
pub async fn for_platform(pool: &PgPool, ctx: &AuthContext, query: HeatmapQuery) -> Result<HeatmapResponse, AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::View)?;
    build(pool, Scope::Platform, query, true).await
}

// GET /classes/{id}/learning-heatmap
pub async fn for_class(pool: &PgPool, ctx: &AuthContext, class_id: Uuid, query: HeatmapQuery) -> Result<HeatmapResponse, AppError> {
    crate::services::org_class::assert_can_manage_class(pool, ctx, class_id).await?;
    let members: Vec<Uuid> = sqlx::query_scalar!(r#"select student_id from class_members where class_id = $1"#, class_id).fetch_all(pool).await?;
    let scope = match query.student_id {
        Some(student) if members.contains(&student) => Scope::Users(vec![student]),
        Some(_) => return Err(AppError::NotFound("student_not_in_class")),
        None => Scope::Users(members),
    };
    build(pool, scope, query, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn grain(path: &[u128], bab: Option<u128>) -> Grain {
        Grain { folder_path: path.iter().map(|n| id(*n)).collect(), bab_id: bab.map(id), difficulty: None, bloom: Some("c1".into()), graded: 1, correct: 1 }
    }

    #[test]
    fn single_child_levels_are_skipped_and_differing_depths_just_work() {
        // root(1) → Matematika(2) → Tahap 1(3) → domain(4) → topik(5) → bab 50
        // root(1) → Matematika(2) → Tahap 2(6) →           topik(7) → bab 70
        let grains = vec![grain(&[1, 2, 3, 4, 5], Some(50)), grain(&[1, 2, 6, 7], Some(70))];
        let (parent, skipped) = settle(&grains, None);
        assert_eq!(parent, Some(id(2)), "root and the only subject are skipped to the Tahap level");
        assert_eq!(skipped, vec![(id(1), NodeKind::Folder), (id(2), NodeKind::Folder)]);
        let mut kids: Vec<_> = grains.iter().filter_map(|g| child_of(g, parent)).collect();
        kids.sort();
        assert_eq!(kids, vec![(id(3), NodeKind::Folder), (id(6), NodeKind::Folder)]);

        // Tahap 2 has a topik directly under it; that topik's children are babs.
        assert_eq!(child_of(&grains[1], Some(id(6))), Some((id(7), NodeKind::Module)));
        assert_eq!(child_of(&grains[1], Some(id(7))), Some((id(70), NodeKind::Bab)));
        // A bab is the leaf, and a grain outside the parent is not counted.
        assert_eq!(child_of(&grains[1], Some(id(70))), None);
        assert_eq!(child_of(&grains[0], Some(id(6))), None);
        // Drilling into Tahap 1 lands on its topik past the lone domain folder,
        // but never skips past a topik into its only bab.
        let (parent, _) = settle(&grains, Some(id(3)));
        assert_eq!(parent, Some(id(5)));
    }
}
