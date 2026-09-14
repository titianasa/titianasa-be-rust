// What Admin Pusat › Pabrik Konten calls: coverage of the library, the
// tree to pick targets from, starting and steering runs, reviewing what
// QA would not pass on its own, curriculum standards, and seeding gold
// examples from the plans Claude already wrote.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::plan::{self, TopicDraft, TopicStatus};
use super::{apply, benchmark, enqueue_task, validate, RunOptions, KINDS};
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::admin_audit;
use crate::services::permissions::{require_permission, Action, Resource};

fn view(ctx: &AuthContext) -> Result<(), AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::View)
}

fn manage(ctx: &AuthContext) -> Result<(), AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::Manage)
}

fn invalid(code: &'static str, detail: impl Into<String>) -> AppError {
    AppError::UnprocessableEntity(code, detail.into())
}

const LIBRARY_ROOT: &str = "Semua Mata Pelajaran";

// ── Coverage ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CoverageRow {
    pub subject_folder_id: Uuid,
    pub subject: String,
    pub tahap_folder_id: Uuid,
    pub tahap: String,
    pub jenjang: Option<String>,
    pub topics: i64,
    pub topics_with_bab: i64,
    pub babs: i64,
    pub babs_with_materi: i64,
    pub tasks_needing_review: i64,
    pub tasks_active: i64,
}

// GET /admin/content-factory/coverage
pub async fn coverage(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<CoverageRow>, AppError> {
    view(ctx)?;
    let rows = sqlx::query!(
        r#"with subj as (
             select s.id, s.title, s.order_index from modules s join modules r on r.id = s.parent_id
             where r.parent_id is null and r.title = $1 and s.is_folder
           ),
           tahap as (
             select t.id, t.title, t.order_index, subj.id as subject_id, subj.title as subject, subj.order_index as subject_order
             from modules t join subj on subj.id = t.parent_id where t.is_folder
           ),
           topics as (
             select tahap.id as tahap_id, m.id as topic_id
             from tahap join modules d on d.parent_id = tahap.id and d.is_folder
             join modules m on m.parent_id = d.id and not m.is_folder and m.source_module_id is null
           ),
           babs as (
             select topics.tahap_id, topics.topic_id, i.id as bab_id,
                    exists (select 1 from module_items a where a.parent_id = i.id and a.content_type = 'article'
                            and jsonb_array_length(coalesce(a.lesson_plan->'sections', '[]')) > 0) as has_materi
             from topics join module_items i on i.module_id = topics.topic_id and i.node_type = 'section'
           )
           select tahap.subject_id as "subject_id!", tahap.subject as "subject!", tahap.id as "tahap_id!", tahap.title as "tahap!",
                  cs.jenjang as "jenjang?",
                  (select count(*) from topics where topics.tahap_id = tahap.id) as "topics!",
                  (select count(distinct topic_id) from babs where babs.tahap_id = tahap.id) as "topics_with_bab!",
                  (select count(*) from babs where babs.tahap_id = tahap.id) as "babs!",
                  (select count(*) from babs where babs.tahap_id = tahap.id and has_materi) as "babs_with_materi!",
                  (select count(*) from generation_tasks gt where gt.status = 'needs_review' and (gt.context->>'tahap_folder_id')::uuid = tahap.id) as "review!",
                  (select count(*) from generation_tasks gt where gt.status in ('queued', 'generating', 'validating', 'qa') and (gt.context->>'tahap_folder_id')::uuid = tahap.id) as "active!"
           from tahap left join curriculum_standards cs on cs.tahap_folder_id = tahap.id
           order by tahap.subject, tahap.order_index, tahap.title"#,
        LIBRARY_ROOT,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CoverageRow {
            subject_folder_id: r.subject_id,
            subject: r.subject,
            tahap_folder_id: r.tahap_id,
            tahap: r.tahap,
            jenjang: r.jenjang,
            topics: r.topics,
            topics_with_bab: r.topics_with_bab,
            babs: r.babs,
            babs_with_materi: r.babs_with_materi,
            tasks_needing_review: r.review,
            tasks_active: r.active,
        })
        .collect())
}

// ── Tree of one tahap, for picking targets ──────────────────────────

#[derive(Debug, Serialize)]
pub struct TreeTopic {
    pub topic_id: Uuid,
    pub title: String,
    pub bab_count: i64,
    pub has_gold: bool,
    /// Of `bab_count`, how many already have a Pembahasan with content —
    /// what the `bab_content` wizard picks topics against.
    pub babs_with_materi: i64,
}

#[derive(Debug, Serialize)]
pub struct TreeDomain {
    pub domain_id: Uuid,
    pub title: String,
    pub topics: Vec<TreeTopic>,
}

#[derive(Debug, Serialize)]
pub struct TahapTree {
    pub tahap_folder_id: Uuid,
    pub tahap: String,
    pub subject: String,
    pub standard: Option<plan::Standard>,
    pub domains: Vec<TreeDomain>,
}

// GET /admin/content-factory/tahap/{id}
pub async fn tahap_tree(pool: &PgPool, ctx: &AuthContext, tahap_folder_id: Uuid) -> Result<TahapTree, AppError> {
    view(ctx)?;
    let head = sqlx::query!(r#"select t.title, s.title as subject from modules t join modules s on s.id = t.parent_id where t.id = $1 and t.is_folder"#, tahap_folder_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("tahap_not_found"))?;
    let standard = sqlx::query!(r#"select jenjang, standar, bahasa, jenis_soal from curriculum_standards where tahap_folder_id = $1"#, tahap_folder_id)
        .fetch_optional(pool)
        .await?
        .map(|r| plan::Standard { jenjang: r.jenjang, standar: r.standar, bahasa: r.bahasa, jenis_soal: r.jenis_soal });
    let rows = sqlx::query!(
        r#"select d.id as domain_id, d.title as domain, m.id as topic_id, m.title as topic,
                  (select count(*) from module_items i where i.module_id = m.id and i.node_type = 'section') as "bab_count!",
                  exists (select 1 from generation_exemplars e where e.domain_folder_id = d.id and e.source = 'claude') as "has_gold!",
                  (select count(*) from module_items i where i.module_id = m.id and i.node_type = 'section'
                     and exists (select 1 from module_items a where a.parent_id = i.id and a.content_type = 'article'
                                 and jsonb_array_length(coalesce(a.lesson_plan->'sections', '[]')) > 0)) as "babs_with_materi!"
           from modules d join modules m on m.parent_id = d.id and not m.is_folder and m.source_module_id is null
           where d.parent_id = $1 and d.is_folder
           order by d.order_index, d.title, m.order_index, m.title"#,
        tahap_folder_id,
    )
    .fetch_all(pool)
    .await?;
    let mut domains: Vec<TreeDomain> = Vec::new();
    for r in rows {
        if domains.last().map(|d| d.domain_id) != Some(r.domain_id) {
            domains.push(TreeDomain { domain_id: r.domain_id, title: r.domain, topics: vec![] });
        }
        domains.last_mut().expect("just pushed").topics.push(TreeTopic { topic_id: r.topic_id, title: r.topic, bab_count: r.bab_count, has_gold: r.has_gold, babs_with_materi: r.babs_with_materi });
    }
    Ok(TahapTree { tahap_folder_id, tahap: head.title, subject: head.subject, standard, domains })
}

// ── Runs ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateRunRequest {
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    /// `bab_plan`: domain folder ids (each becomes one task, its topics
    /// planned together). `bab_content`: TOPIC ids instead — content is
    /// generated per bab, so there's no domain-wide context to share,
    /// and each topic's babs missing materi become their own tasks.
    pub domain_ids: Vec<Uuid>,
    /// `bab_plan`: restrict to these topics within `domain_ids` (default:
    /// every topic without babs). `bab_content`: restrict to these
    /// specific bab (section) ids instead of every bab still missing
    /// materi across `domain_ids`'s topics.
    #[serde(default)]
    pub topic_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    pub options: RunOptions,
    #[serde(default)]
    pub token_limit: Option<i64>,
    #[serde(default)]
    pub benchmark: bool,
}

#[derive(Debug, Serialize)]
pub struct RunRow {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub benchmark: bool,
    pub total_tasks: i32,
    pub counts: HashMap<String, i64>,
    pub tokens_used: i64,
    pub token_limit: Option<i64>,
    pub pause_reason: Option<String>,
    pub options: Value,
    pub created_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct CreateRunResponse {
    pub run: RunRow,
    pub skipped_domains: Vec<String>,
}

// POST /admin/content-factory/runs
pub async fn create_run(pool: &PgPool, ctx: &AuthContext, req: CreateRunRequest) -> Result<CreateRunResponse, AppError> {
    manage(ctx)?;
    if !KINDS.contains(&req.kind.as_str()) {
        return Err(invalid("invalid_kind", format!("jenis \"{}\" belum didukung", req.kind)));
    }
    if req.domain_ids.is_empty() {
        return Err(invalid("empty_scope", "pilih minimal satu domain atau topik"));
    }
    if !(1.0..=5.0).contains(&req.options.qa_threshold) || !(0..=4).contains(&req.options.max_repairs) {
        return Err(invalid("invalid_options", "ambang QA 1–5 dan perbaikan 0–4"));
    }
    if req.kind == "bab_content" {
        return create_bab_content_run(pool, ctx, req).await;
    }

    let mut scope_domains = Vec::new();
    let mut skipped = Vec::new();
    let mut labels = Vec::new();
    for domain_id in &req.domain_ids {
        let ctx_result = plan::load_context(pool, *domain_id, req.topic_ids.as_deref()).await;
        let domain = match ctx_result {
            Ok(d) => d,
            Err(AppError::UnprocessableEntity("standard_missing", detail)) => {
                skipped.push(detail);
                continue;
            }
            Err(e) => return Err(e),
        };
        let targets: Vec<Uuid> = if req.benchmark {
            let gold = sqlx::query_scalar!(r#"select output from generation_exemplars where kind = 'bab_plan' and domain_folder_id = $1 and source = 'claude'"#, domain_id).fetch_optional(pool).await?;
            match gold {
                Some(out) => out.get("topik").and_then(Value::as_array).map(|a| a.iter().filter_map(|t| t.get("topic_id")?.as_str()?.parse().ok()).collect()).unwrap_or_default(),
                None => {
                    skipped.push(format!("{}: belum punya rencana acuan untuk benchmark", domain.domain_title));
                    continue;
                }
            }
        } else {
            domain.target_topic_ids.clone()
        };
        if targets.is_empty() {
            skipped.push(format!("{}: semua topik sudah punya bab", domain.domain_title));
            continue;
        }
        labels.push((domain.domain_folder_id, format!("{} › {} ({} topik)", domain.tahap_title, domain.domain_title, targets.len())));
        scope_domains.push(json!({"domain_id": domain.domain_folder_id, "topic_ids": targets}));
    }
    if scope_domains.is_empty() {
        return Err(invalid("nothing_to_generate", format!("tidak ada yang bisa digenerate: {}", skipped.join("; "))));
    }

    let title = req.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| format!("{} {} domain", if req.benchmark { "Benchmark" } else { "Susun bab" }, scope_domains.len()));
    let run_id = sqlx::query_scalar!(
        r#"insert into generation_runs (kind, title, scope, options, benchmark, total_tasks, token_limit, created_by)
           values ($1, $2, $3, $4, $5, $6, $7, $8) returning id"#,
        req.kind,
        title,
        json!({"domains": scope_domains}),
        serde_json::to_value(&req.options).unwrap_or(Value::Null),
        req.benchmark,
        scope_domains.len() as i32,
        req.token_limit,
        ctx.user_id,
    )
    .fetch_one(pool)
    .await?;
    for (domain_id, label) in labels {
        let task_id = sqlx::query_scalar!(r#"insert into generation_tasks (run_id, kind, target_type, target_id, label) values ($1, $2, 'domain', $3, $4) returning id"#, run_id, req.kind, domain_id, label)
            .fetch_one(pool)
            .await?;
        enqueue_task(pool, task_id, if req.benchmark { 50 } else { 100 }).await?;
    }
    admin_audit::record(pool, Some(ctx.user_id), "content_factory.run_created", "generation_run", Some(run_id), None, Some(json!({"title": title, "domains": req.domain_ids.len(), "benchmark": req.benchmark})), None).await?;
    Ok(CreateRunResponse { run: get_run_row(pool, run_id).await?, skipped_domains: skipped })
}

/// `bab_content`: `req.domain_ids` are topic ids; each topic's babs
/// still missing materi (no `lesson_plan.sections`) become their own
/// task — content is generated and applied directly per bab (see
/// `content_runner::run_bab_content`), so there's no domain-wide plan to
/// share the way `bab_plan` has. Benchmark mode isn't wired here yet
/// (see `bin/benchmark_bab_content.rs`).
async fn create_bab_content_run(pool: &PgPool, ctx: &AuthContext, req: CreateRunRequest) -> Result<CreateRunResponse, AppError> {
    if req.benchmark {
        return Err(invalid("benchmark_not_supported", "mode benchmark untuk isi konten belum ada di dashboard — jalankan `cargo run --bin benchmark_bab_content` dari server"));
    }
    let mut targets: Vec<(Uuid, String)> = Vec::new();
    let mut skipped = Vec::new();
    for topic_id in &req.domain_ids {
        let topic = sqlx::query!(r#"select title from modules where id = $1 and not is_folder"#, topic_id).fetch_optional(pool).await?;
        let Some(topic) = topic else {
            skipped.push(format!("topik tidak ditemukan: {topic_id}"));
            continue;
        };
        let babs = sqlx::query!(
            r#"select i.id, i.title,
                      exists (select 1 from module_items a where a.parent_id = i.id and a.content_type = 'article'
                              and jsonb_array_length(coalesce(a.lesson_plan->'sections', '[]')) > 0) as "has_materi!"
               from module_items i where i.module_id = $1 and i.node_type = 'section' order by i.order_index"#,
            topic_id,
        )
        .fetch_all(pool)
        .await?;
        let topic_targets: Vec<(Uuid, String)> = babs
            .iter()
            .filter(|b| !b.has_materi)
            .filter(|b| req.topic_ids.as_ref().is_none_or(|allow| allow.contains(&b.id)))
            .map(|b| (b.id, format!("{} › {}", topic.title, b.title)))
            .collect();
        if topic_targets.is_empty() {
            skipped.push(format!("{}: semua bab sudah punya materi (atau di luar pilihan bab)", topic.title));
            continue;
        }
        targets.extend(topic_targets);
    }
    if targets.is_empty() {
        return Err(invalid("nothing_to_generate", format!("tidak ada yang bisa digenerate: {}", skipped.join("; "))));
    }

    let title = req.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| format!("Isi konten {} bab", targets.len()));
    let run_id = sqlx::query_scalar!(
        r#"insert into generation_runs (kind, title, scope, options, benchmark, total_tasks, token_limit, created_by)
           values ($1, $2, $3, $4, false, $5, $6, $7) returning id"#,
        req.kind,
        title,
        json!({"babs": targets.iter().map(|(id, _)| id).collect::<Vec<_>>()}),
        serde_json::to_value(&req.options).unwrap_or(Value::Null),
        targets.len() as i32,
        req.token_limit,
        ctx.user_id,
    )
    .fetch_one(pool)
    .await?;
    for (bab_id, label) in &targets {
        let task_id = sqlx::query_scalar!(r#"insert into generation_tasks (run_id, kind, target_type, target_id, label) values ($1, $2, 'bab', $3, $4) returning id"#, run_id, req.kind, bab_id, label)
            .fetch_one(pool)
            .await?;
        enqueue_task(pool, task_id, 100).await?;
    }
    admin_audit::record(pool, Some(ctx.user_id), "content_factory.run_created", "generation_run", Some(run_id), None, Some(json!({"title": title, "babs": targets.len()})), None).await?;
    Ok(CreateRunResponse { run: get_run_row(pool, run_id).await?, skipped_domains: skipped })
}

async fn get_run_row(pool: &PgPool, run_id: Uuid) -> Result<RunRow, AppError> {
    let r = sqlx::query!(
        r#"select g.id, g.kind, g.title, g.status, g.benchmark, g.total_tasks, g.tokens_used, g.token_limit, g.pause_reason, g.options, g.created_at, g.started_at, g.finished_at,
                  u.name as "created_by_name?",
                  coalesce((select jsonb_object_agg(status, n) from (select status, count(*) as n from generation_tasks where run_id = g.id group by status) c), '{}'::jsonb) as "counts!"
           from generation_runs g left join users u on u.id = g.created_by where g.id = $1"#,
        run_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("generation_run_not_found"))?;
    Ok(RunRow {
        id: r.id,
        kind: r.kind,
        title: r.title,
        status: r.status,
        benchmark: r.benchmark,
        total_tasks: r.total_tasks,
        counts: serde_json::from_value(r.counts).unwrap_or_default(),
        tokens_used: r.tokens_used,
        token_limit: r.token_limit,
        pause_reason: r.pause_reason,
        options: r.options,
        created_by_name: r.created_by_name,
        created_at: r.created_at,
        started_at: r.started_at,
        finished_at: r.finished_at,
    })
}

// GET /admin/content-factory/runs
pub async fn list_runs(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<RunRow>, AppError> {
    view(ctx)?;
    let ids = sqlx::query_scalar!(r#"select id from generation_runs order by created_at desc limit 100"#).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(get_run_row(pool, id).await?);
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct TaskSummary {
    pub id: Uuid,
    pub label: String,
    pub status: String,
    pub attempt: i32,
    pub topics: i64,
    pub topics_applied: i64,
    pub topics_review: i64,
    pub qa_average: Option<f64>,
    pub tokens: i64,
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub run: RunRow,
    pub tasks: Vec<TaskSummary>,
    pub benchmark: Option<benchmark::BenchmarkReport>,
}

/// `bab_plan`'s `qa` is an array of per-topic `TopicScore`; `bab_content`'s
/// is a single `ContentScore` object (one bab, not many topics). Tries
/// both shapes so callers don't need to know which kind a task is.
fn qa_average(qa: &Option<Value>) -> Option<f64> {
    let value = qa.as_ref()?;
    if let Some(list) = value.as_array() {
        let scores: Vec<f64> = list.iter().filter_map(|s| serde_json::from_value::<super::qa::TopicScore>(s.clone()).ok()).map(|s| s.average()).collect();
        return (!scores.is_empty()).then(|| scores.iter().sum::<f64>() / scores.len() as f64);
    }
    serde_json::from_value::<super::content_qa::ContentScore>(value.clone()).ok().map(|s| s.average())
}

// GET /admin/content-factory/runs/{id}
pub async fn get_run(pool: &PgPool, ctx: &AuthContext, run_id: Uuid) -> Result<RunDetail, AppError> {
    view(ctx)?;
    let run = get_run_row(pool, run_id).await?;
    let rows = sqlx::query!(r#"select id, label, status, attempt, draft, qa, tokens_generate, tokens_qa, error, updated_at from generation_tasks where run_id = $1 order by created_at"#, run_id)
        .fetch_all(pool)
        .await?;
    let tasks = rows
        .into_iter()
        .map(|r| {
            let drafts: Vec<TopicDraft> = r.draft.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
            TaskSummary {
                id: r.id,
                label: r.label,
                status: r.status,
                attempt: r.attempt,
                topics: drafts.len() as i64,
                topics_applied: drafts.iter().filter(|d| d.status == TopicStatus::Applied).count() as i64,
                topics_review: drafts.iter().filter(|d| d.status == TopicStatus::NeedsReview).count() as i64,
                qa_average: qa_average(&r.qa),
                tokens: r.tokens_generate + r.tokens_qa,
                error: r.error,
                updated_at: r.updated_at,
            }
        })
        .collect();
    let report = if run.benchmark { Some(benchmark::run_report(pool, run_id).await?) } else { None };
    Ok(RunDetail { run, tasks, benchmark: report })
}

#[derive(Debug, Deserialize)]
pub struct RunActionRequest {
    /// pause | resume | cancel | retry_failed
    pub action: String,
}

// POST /admin/content-factory/runs/{id}/action
pub async fn run_action(pool: &PgPool, ctx: &AuthContext, run_id: Uuid, req: RunActionRequest) -> Result<RunRow, AppError> {
    manage(ctx)?;
    let run = get_run_row(pool, run_id).await?;
    match req.action.as_str() {
        "pause" => {
            sqlx::query!(r#"update generation_runs set status = 'paused', pause_reason = 'dijeda admin', updated_at = now() where id = $1 and status in ('queued', 'running')"#, run_id).execute(pool).await?;
        }
        "resume" => {
            if run.status != "paused" {
                return Err(AppError::Conflict("run_not_paused"));
            }
            sqlx::query!(r#"update generation_runs set status = 'running', pause_reason = null, updated_at = now() where id = $1"#, run_id).execute(pool).await?;
            requeue(pool, run_id, "queued").await?;
        }
        "cancel" => {
            sqlx::query!(r#"update generation_runs set status = 'cancelled', finished_at = now(), updated_at = now() where id = $1 and status not in ('done', 'cancelled')"#, run_id).execute(pool).await?;
            sqlx::query!(r#"update generation_tasks set status = 'cancelled', updated_at = now() where run_id = $1 and status = 'queued'"#, run_id).execute(pool).await?;
        }
        "retry_failed" => {
            sqlx::query!(r#"update generation_tasks set status = 'queued', attempt = 0, error = null, updated_at = now() where run_id = $1 and status = 'failed'"#, run_id).execute(pool).await?;
            sqlx::query!(r#"update generation_runs set status = 'running', finished_at = null, updated_at = now() where id = $1"#, run_id).execute(pool).await?;
            requeue(pool, run_id, "queued").await?;
        }
        _ => return Err(invalid("invalid_action", "aksi harus pause, resume, cancel, atau retry_failed")),
    }
    admin_audit::record(pool, Some(ctx.user_id), &format!("content_factory.run_{}", req.action), "generation_run", Some(run_id), Some(json!({"status": run.status})), None, None).await?;
    get_run_row(pool, run_id).await
}

/// Enqueues tasks in `status` that have no live job.
async fn requeue(pool: &PgPool, run_id: Uuid, status: &str) -> Result<(), AppError> {
    let ids = sqlx::query_scalar!(
        r#"select t.id from generation_tasks t where t.run_id = $1 and t.status = $2
             and not exists (select 1 from jobs j where j.id = t.job_id and j.status in ('pending', 'running'))"#,
        run_id,
        status,
    )
    .fetch_all(pool)
    .await?;
    for id in ids {
        enqueue_task(pool, id, 100).await?;
    }
    Ok(())
}

// ── Tasks & review ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct TaskDetail {
    pub id: Uuid,
    pub run_id: Uuid,
    pub run_title: String,
    pub kind: String,
    pub label: String,
    pub status: String,
    pub attempt: i32,
    pub prompt_version: Option<String>,
    pub context: Option<Value>,
    pub draft: Option<Value>,
    pub validation: Option<Value>,
    pub qa: Option<Value>,
    pub history: Value,
    pub tokens_generate: i64,
    pub tokens_qa: i64,
    pub error: Option<String>,
    pub review_note: Option<String>,
    pub reviewed_by_name: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

// GET /admin/content-factory/tasks/{id}
pub async fn get_task(pool: &PgPool, ctx: &AuthContext, task_id: Uuid) -> Result<TaskDetail, AppError> {
    view(ctx)?;
    let r = sqlx::query!(
        r#"select t.id, t.run_id, g.title as run_title, t.kind, t.label, t.status, t.attempt, t.prompt_version, t.context, t.draft, t.validation, t.qa, t.history,
                  t.tokens_generate, t.tokens_qa, t.error, t.review_note, u.name as "reviewed_by_name?", t.reviewed_at, t.updated_at
           from generation_tasks t join generation_runs g on g.id = t.run_id left join users u on u.id = t.reviewed_by where t.id = $1"#,
        task_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("generation_task_not_found"))?;
    Ok(TaskDetail {
        id: r.id,
        run_id: r.run_id,
        run_title: r.run_title,
        kind: r.kind,
        label: r.label,
        status: r.status,
        attempt: r.attempt,
        prompt_version: r.prompt_version,
        context: r.context,
        draft: r.draft,
        validation: r.validation,
        qa: r.qa,
        history: r.history,
        tokens_generate: r.tokens_generate,
        tokens_qa: r.tokens_qa,
        error: r.error,
        review_note: r.review_note,
        reviewed_by_name: r.reviewed_by_name,
        reviewed_at: r.reviewed_at,
        updated_at: r.updated_at,
    })
}

#[derive(Debug, Serialize)]
pub struct ReviewQueueRow {
    pub id: Uuid,
    pub kind: String,
    pub run_title: String,
    pub label: String,
    pub topics_review: i64,
    pub qa_average: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

// GET /admin/content-factory/review
pub async fn review_queue(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<ReviewQueueRow>, AppError> {
    view(ctx)?;
    let rows = sqlx::query!(
        r#"select t.id, t.kind, g.title as run_title, t.label, t.draft, t.qa, t.updated_at from generation_tasks t join generation_runs g on g.id = t.run_id
           where t.status = 'needs_review' order by t.updated_at asc limit 200"#
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let drafts: Vec<TopicDraft> = r.draft.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
            ReviewQueueRow {
                id: r.id,
                kind: r.kind,
                run_title: r.run_title,
                label: r.label,
                topics_review: drafts.iter().filter(|d| d.status == TopicStatus::NeedsReview).count() as i64,
                qa_average: qa_average(&r.qa),
                updated_at: r.updated_at,
            }
        })
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct ReviewRequest {
    /// approve | reject | regenerate
    pub action: String,
    /// Which topics (default: all that need review).
    #[serde(default)]
    pub topic_ids: Option<Vec<Uuid>>,
    /// A reviewer's edited plans, applied instead of the draft.
    #[serde(default)]
    pub edited: Option<Vec<TopicDraft>>,
    #[serde(default)]
    pub note: Option<String>,
}

// POST /admin/content-factory/tasks/{id}/review
pub async fn review_task(pool: &PgPool, ctx: &AuthContext, task_id: Uuid, req: ReviewRequest) -> Result<TaskDetail, AppError> {
    manage(ctx)?;
    let task = sqlx::query!(r#"select run_id, status, context, draft, history from generation_tasks where id = $1"#, task_id).fetch_optional(pool).await?.ok_or(AppError::NotFound("generation_task_not_found"))?;
    if task.status != "needs_review" {
        return Err(AppError::Conflict("task_not_in_review"));
    }
    let domain: plan::DomainContext = task.context.and_then(|v| serde_json::from_value(v).ok()).ok_or_else(|| AppError::Internal(anyhow::anyhow!("review task has no context")))?;
    let mut drafts: Vec<TopicDraft> = task.draft.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    // Topics the plan never returned at all still need a decision.
    for t in domain.targets() {
        if !drafts.iter().any(|d| d.topic_id == t.topic_id) {
            drafts.push(TopicDraft { topic_id: t.topic_id, t: t.title.clone(), jenis_soal: None, bab: vec![], status: TopicStatus::NeedsReview });
        }
    }
    let chosen: Vec<Uuid> = drafts.iter().filter(|d| d.status == TopicStatus::NeedsReview && req.topic_ids.as_ref().is_none_or(|ids| ids.contains(&d.topic_id))).map(|d| d.topic_id).collect();
    if chosen.is_empty() {
        return Err(invalid("nothing_selected", "tidak ada topik yang menunggu tinjauan di pilihan ini"));
    }
    let note = req.note.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);

    match req.action.as_str() {
        "approve" => {
            if let Some(edited) = &req.edited {
                for e in edited.iter().filter(|e| chosen.contains(&e.topic_id)) {
                    if let Some(d) = drafts.iter_mut().find(|d| d.topic_id == e.topic_id) {
                        d.bab = e.bab.clone();
                        d.jenis_soal = e.jenis_soal.clone();
                    }
                }
            }
            let targets: Vec<&plan::TopicContext> = domain.targets().into_iter().filter(|t| chosen.contains(&t.topic_id)).collect();
            let chosen_drafts: Vec<TopicDraft> = drafts.iter().filter(|d| chosen.contains(&d.topic_id)).cloned().collect();
            let blockers: Vec<String> = validate::validate(&targets, &chosen_drafts, &domain.standard.jenjang).into_iter().filter(|i| i.severity == validate::Severity::Blocker).map(|i| i.message).collect();
            if !blockers.is_empty() {
                return Err(invalid("plan_has_blockers", format!("perbaiki dulu: {}", blockers.join("; "))));
            }
            for d in drafts.iter_mut().filter(|d| chosen.contains(&d.topic_id)) {
                apply::apply_topic(pool, ctx, &domain, d, task_id, None, "gemini+review").await?;
                d.status = TopicStatus::Applied;
            }
        }
        "reject" => {
            for d in drafts.iter_mut().filter(|d| chosen.contains(&d.topic_id)) {
                d.status = TopicStatus::Rejected;
            }
        }
        "regenerate" => {
            for d in drafts.iter_mut().filter(|d| chosen.contains(&d.topic_id)) {
                d.status = TopicStatus::Pending;
            }
            let mut history = task.history;
            if let Some(list) = history.as_array_mut() {
                list.push(json!({"kind": "reviewer_note", "at": Utc::now(), "detail": {"note": note, "topics": chosen}}));
            }
            sqlx::query!(
                r#"update generation_tasks set status = 'queued', attempt = 0, draft = $2, history = $3, review_note = $4, reviewed_by = $5, reviewed_at = now(), updated_at = now() where id = $1"#,
                task_id,
                serde_json::to_value(&drafts).unwrap_or(Value::Null),
                history,
                note,
                ctx.user_id,
            )
            .execute(pool)
            .await?;
            sqlx::query!(r#"update generation_runs set status = 'running', finished_at = null, updated_at = now() where id = $1 and status in ('done', 'running', 'queued')"#, task.run_id).execute(pool).await?;
            enqueue_task(pool, task_id, 60).await?;
            admin_audit::record(pool, Some(ctx.user_id), "content_factory.task_regenerate", "generation_task", Some(task_id), None, Some(json!({"topics": chosen})), note.as_deref()).await?;
            return get_task(pool, ctx, task_id).await;
        }
        _ => return Err(invalid("invalid_action", "aksi harus approve, reject, atau regenerate")),
    }

    let remaining = drafts.iter().any(|d| d.status == TopicStatus::NeedsReview);
    let status = if remaining {
        "needs_review"
    } else if drafts.iter().any(|d| d.status == TopicStatus::Applied) {
        "applied"
    } else {
        "rejected"
    };
    sqlx::query!(
        r#"update generation_tasks set status = $2, draft = $3, review_note = coalesce($4, review_note), reviewed_by = $5, reviewed_at = now(), updated_at = now() where id = $1"#,
        task_id,
        status,
        serde_json::to_value(&drafts).unwrap_or(Value::Null),
        note,
        ctx.user_id,
    )
    .execute(pool)
    .await?;
    admin_audit::record(pool, Some(ctx.user_id), &format!("content_factory.task_{}", req.action), "generation_task", Some(task_id), None, Some(json!({"topics": chosen})), note.as_deref()).await?;
    get_task(pool, ctx, task_id).await
}

// ── Curriculum standards ────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StandardRow {
    pub tahap_folder_id: Uuid,
    pub subject: String,
    pub tahap: String,
    pub standard: Option<plan::Standard>,
    pub catatan: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

// GET /admin/content-factory/standards
pub async fn list_standards(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<StandardRow>, AppError> {
    view(ctx)?;
    let rows = sqlx::query!(
        r#"select t.id, s.title as subject, t.title as tahap, cs.jenjang as "jenjang?", cs.standar as "standar?", cs.bahasa as "bahasa?", cs.jenis_soal as "jenis_soal?", cs.catatan, cs.updated_at as "updated_at?"
           from modules t join modules s on s.id = t.parent_id join modules r on r.id = s.parent_id
           left join curriculum_standards cs on cs.tahap_folder_id = t.id
           where r.parent_id is null and r.title = $1 and t.is_folder and s.is_folder
           order by s.title, t.order_index, t.title"#,
        LIBRARY_ROOT,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| StandardRow {
            tahap_folder_id: r.id,
            subject: r.subject,
            tahap: r.tahap,
            standard: r.jenjang.map(|jenjang| plan::Standard { jenjang, standar: r.standar.unwrap_or_default(), bahasa: r.bahasa.unwrap_or_else(|| "id".into()), jenis_soal: r.jenis_soal.unwrap_or_else(|| "hitungan".into()) }),
            catatan: r.catatan,
            updated_at: r.updated_at,
        })
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct UpsertStandardRequest {
    pub jenjang: String,
    #[serde(default)]
    pub standar: Vec<String>,
    #[serde(default)]
    pub bahasa: Option<String>,
    #[serde(default)]
    pub jenis_soal: Option<String>,
    #[serde(default)]
    pub catatan: Option<String>,
}

// PUT /admin/content-factory/standards/{tahap_id}
pub async fn upsert_standard(pool: &PgPool, ctx: &AuthContext, tahap_folder_id: Uuid, req: UpsertStandardRequest) -> Result<StandardRow, AppError> {
    manage(ctx)?;
    let jenjang = req.jenjang.trim();
    if jenjang.is_empty() {
        return Err(invalid("jenjang_required", "jenjang wajib diisi (misal \"SMA/MA kelas 11 (Fase F)\")"));
    }
    let jenis_soal = req.jenis_soal.unwrap_or_else(|| "hitungan".into());
    if !plan::JENIS_SOAL.contains(&jenis_soal.as_str()) {
        return Err(invalid("invalid_jenis_soal", "jenis soal harus hitungan, konsep, atau bahasa"));
    }
    let standar: Vec<String> = req.standar.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let subject_id = sqlx::query_scalar!(r#"select m.subject_id from modules d join modules m on m.parent_id = d.id where d.parent_id = $1 and m.subject_id is not null limit 1"#, tahap_folder_id).fetch_optional(pool).await?.flatten();
    sqlx::query!(
        r#"insert into curriculum_standards (tahap_folder_id, subject_id, jenjang, standar, bahasa, jenis_soal, catatan, updated_by)
           values ($1, $2, $3, $4, $5, $6, $7, $8)
           on conflict (tahap_folder_id) do update set jenjang = excluded.jenjang, standar = excluded.standar, bahasa = excluded.bahasa,
             jenis_soal = excluded.jenis_soal, catatan = excluded.catatan, updated_by = excluded.updated_by, updated_at = now()"#,
        tahap_folder_id,
        subject_id,
        jenjang,
        &standar,
        req.bahasa.unwrap_or_else(|| "id".into()),
        jenis_soal,
        req.catatan,
        ctx.user_id,
    )
    .execute(pool)
    .await?;
    admin_audit::record(pool, Some(ctx.user_id), "content_factory.standard_saved", "curriculum_standard", Some(tahap_folder_id), None, Some(json!({"jenjang": jenjang})), None).await?;
    list_standards(pool, ctx).await?.into_iter().find(|s| s.tahap_folder_id == tahap_folder_id).ok_or(AppError::NotFound("tahap_not_found"))
}

// ── Seeding from the plans Claude wrote ─────────────────────────────

#[derive(Debug, Serialize)]
pub struct SeedReport {
    pub standards_created: u64,
    pub exemplars_upserted: u64,
}

// POST /admin/content-factory/seed
/// Standards and gold examples from every topic whose `bab_plan` was
/// written by Claude. Existing standards are never overwritten (an admin
/// may have edited them); exemplars are refreshed.
pub async fn seed_from_metadata(pool: &PgPool, ctx: &AuthContext) -> Result<SeedReport, AppError> {
    manage(ctx)?;
    let rows = sqlx::query!(
        r#"select m.id as topic_id, m.title as topic, m.subject_id, m.metadata->'bab_plan' as "plan!", d.id as domain_id, d.title as domain,
                  t.id as tahap_id, t.title as tahap, s.title as subject_folder, m.order_index
           from modules m join modules d on d.id = m.parent_id join modules t on t.id = d.parent_id join modules s on s.id = t.parent_id
           where m.source_module_id is null and not m.is_folder and m.metadata->'bab_plan'->>'author' = 'claude'
           order by d.id, m.order_index, m.title"#
    )
    .fetch_all(pool)
    .await?;

    let mut standards_created = 0;
    let mut by_tahap: HashMap<Uuid, Vec<&Value>> = HashMap::new();
    for r in &rows {
        by_tahap.entry(r.tahap_id).or_default().push(&r.plan);
    }
    for (tahap_id, plans) in &by_tahap {
        let mode = |key: &str| {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for p in plans {
                if let Some(v) = p.get(key).and_then(Value::as_str) {
                    *counts.entry(v.to_string()).or_default() += 1;
                }
            }
            counts.into_iter().max_by_key(|(_, n)| *n).map(|(v, _)| v)
        };
        let Some(jenjang) = mode("jenjang") else { continue };
        let standar: Vec<String> = plans.first().and_then(|p| p.get("standar")).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        let subject_id = rows.iter().find(|r| r.tahap_id == *tahap_id).and_then(|r| r.subject_id);
        let result = sqlx::query!(
            r#"insert into curriculum_standards (tahap_folder_id, subject_id, jenjang, standar, bahasa, jenis_soal, updated_by)
               values ($1, $2, $3, $4, $5, $6, $7) on conflict (tahap_folder_id) do nothing"#,
            tahap_id,
            subject_id,
            jenjang,
            &standar,
            mode("bahasa").unwrap_or_else(|| "id".into()),
            mode("jenis_soal").unwrap_or_else(|| "hitungan".into()),
            ctx.user_id,
        )
        .execute(pool)
        .await?;
        standards_created += result.rows_affected();
    }

    let mut exemplars_upserted = 0;
    let mut i = 0;
    while i < rows.len() {
        let domain_id = rows[i].domain_id;
        let group: Vec<_> = rows[i..].iter().take_while(|r| r.domain_id == domain_id).collect();
        i += group.len();
        let first = group[0];
        let jenjang = first.plan.get("jenjang").and_then(Value::as_str).unwrap_or_default().to_string();
        let input = json!({
            "mapel": first.subject_folder,
            "tahap": first.tahap,
            "jenjang": jenjang,
            "domain": first.domain,
            "topik": group.iter().enumerate().map(|(n, r)| json!({"no": n + 1, "t": r.topic, "dipakai_di": r.plan.get("jalur")})).collect::<Vec<_>>(),
        });
        let output = json!({
            "topik": group.iter().enumerate().map(|(n, r)| json!({"no": n + 1, "topic_id": r.topic_id, "t": r.topic, "jenis_soal": r.plan.get("jenis_soal"), "bab": r.plan.get("bab")})).collect::<Vec<_>>(),
        });
        let result = sqlx::query!(
            r#"insert into generation_exemplars (kind, subject_id, stage, domain_folder_id, title, input, output, source)
               values ('bab_plan', $1, $2, $3, $4, $5, $6, 'claude')
               on conflict (kind, domain_folder_id, source) do update set input = excluded.input, output = excluded.output, stage = excluded.stage, title = excluded.title"#,
            first.subject_id,
            plan::stage_key(&jenjang),
            domain_id,
            format!("{} › {} › {}", first.subject_folder, first.tahap, first.domain),
            input,
            output,
        )
        .execute(pool)
        .await?;
        exemplars_upserted += result.rows_affected();
    }
    admin_audit::record(pool, Some(ctx.user_id), "content_factory.seeded", "generation_exemplar", None, None, Some(json!({"standards": standards_created, "exemplars": exemplars_upserted})), None).await?;
    Ok(SeedReport { standards_created, exemplars_upserted })
}

#[derive(Debug, Serialize)]
pub struct ExemplarRow {
    pub id: Uuid,
    pub title: String,
    pub stage: Option<String>,
    pub source: String,
    pub topics: i64,
    pub enabled: bool,
    pub domain_folder_id: Option<Uuid>,
}

// GET /admin/content-factory/exemplars
pub async fn list_exemplars(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<ExemplarRow>, AppError> {
    view(ctx)?;
    let rows = sqlx::query!(
        r#"select id, title, stage, source, enabled, domain_folder_id, coalesce(jsonb_array_length(output->'topik'), 0) as "topics!"
           from generation_exemplars where kind = 'bab_plan' order by title"#
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| ExemplarRow { id: r.id, title: r.title, stage: r.stage, source: r.source, topics: r.topics as i64, enabled: r.enabled, domain_folder_id: r.domain_folder_id }).collect())
}
