// One `bab_content` task through the factory: load the bab's brief,
// generate straight into its real items (materi + checkpoints + bank —
// `content::generate_bab` already writes as it goes, unlike `bab_plan`
// which drafts before applying), validate the result by rule, judge it,
// and decide applied vs needs_review. Runs inside a `content_generation`
// job, same as `runner::run_bab_plan`.
//
// Benchmark mode is deliberately NOT supported here yet — that ran once,
// for real, as `bin/benchmark_bab_content.rs` (see the project memory),
// and wiring it into this run/task shape is follow-up work, not what
// this pass needed. `admin::create_run` refuses `benchmark: true` for
// this kind with a message pointing at the script.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::{content, content_qa, RunOptions};
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::GenerationRequest;
use crate::services::job_queue::JobContext;
use crate::services::storage::InMemoryStorage;

const LESSON_ROLE: &str = "lesson_generation";
const QUIZ_ROLE: &str = "quiz_generation";
const JUDGE_ROLE: &str = "agent_qa";
const SAMPLE_SIZE: usize = 8;

#[derive(Debug)]
pub enum Outcome {
    Done,
    /// Put back to `queued` without spending an attempt (run paused).
    Deferred,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentTaskContext {
    pub bab_id: Uuid,
    pub topic_id: Uuid,
    pub topic_title: String,
    pub bab_title: String,
    pub level: String,
    pub language: String,
    pub jenis_soal: String,
    pub siblings: Vec<String>,
    pub position: usize,
    pub notes: String,
    pub article_id: Uuid,
    pub bank_id: Uuid,
}

/// The tujuan/cakupan for one bab, from the topic's `metadata.bab_plan`
/// (written by the `bab_plan` kind) — empty when the topic predates
/// that pipeline, same as Python's `bab_brief()`.
fn bab_brief(bab_plan: Option<&Value>, bab_title: &str) -> String {
    let Some(plan) = bab_plan else { return String::new() };
    let mut lines = Vec::new();
    if let Some(entry) = plan.get("bab").and_then(Value::as_array).and_then(|arr| arr.iter().find(|b| b.get("judul").and_then(Value::as_str) == Some(bab_title))) {
        if let Some(tujuan) = entry.get("tujuan").and_then(Value::as_str) {
            lines.push(format!("Tujuan bab: {tujuan}"));
        }
        if let Some(cakupan) = entry.get("cakupan").and_then(Value::as_array) {
            let items: Vec<String> = cakupan.iter().filter_map(Value::as_str).map(|c| format!("  - {c}")).collect();
            if !items.is_empty() {
                lines.push(format!("Cakupan WAJIB (semua harus diajarkan, jangan melebar di luar ini):\n{}", items.join("\n")));
            }
        }
    }
    if let Some(items) = plan.get("standar").and_then(Value::as_array) {
        let items: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
        if !items.is_empty() {
            lines.push(format!("Acuan standar: {}", items.join("; ")));
        }
    }
    lines.join("\n")
}

async fn load_context(pool: &PgPool, bab_id: Uuid) -> Result<ContentTaskContext, AppError> {
    let bab = sqlx::query!(r#"select title as "title!", module_id as "topic_id!" from module_items where id = $1 and node_type = 'section'"#, bab_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("bab_not_found"))?;
    let topic = sqlx::query!(
        r#"select m.title as "topic_title!", m.metadata, t.id as "tahap_id!", t.title as "tahap_title!"
           from modules m join modules d on d.id = m.parent_id join modules t on t.id = d.parent_id
           where m.id = $1"#,
        bab.topic_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("topic_not_found"))?;
    let standard = sqlx::query!(r#"select jenjang, bahasa, jenis_soal from curriculum_standards where tahap_folder_id = $1"#, topic.tahap_id).fetch_optional(pool).await?;

    let siblings_rows = sqlx::query!(r#"select id, title from module_items where module_id = $1 and parent_id is null and node_type = 'section' order by order_index"#, bab.topic_id).fetch_all(pool).await?;
    let position = siblings_rows.iter().position(|s| s.id == bab_id).ok_or_else(|| AppError::Internal(anyhow::anyhow!("bab tidak ditemukan di antara saudaranya sendiri")))?;
    let siblings: Vec<String> = siblings_rows.iter().map(|s| s.title.clone()).collect();

    let children = sqlx::query!(r#"select id, content_type, quiz_config from module_items where parent_id = $1"#, bab_id).fetch_all(pool).await?;
    let article_id = children
        .iter()
        .find(|c| c.content_type.as_deref() == Some("article"))
        .map(|c| c.id)
        .ok_or_else(|| AppError::UnprocessableEntity("article_missing", "bab ini belum punya item Pembahasan — jalankan Susun Bab dulu".to_string()))?;
    let quiz_children: Vec<_> = children.iter().filter(|c| c.content_type.as_deref() == Some("quiz")).collect();
    // The bank is whichever quiz item does NOT draw from another item
    // (`question_pool` unset — including simply absent, as on a bab this
    // old, from before Latihan 1/2/3 pooling existed and a bab got one
    // "Latihan" item instead of three). Falls back to "the only quiz
    // item there is" for that older shape; `generate_bab` below then
    // brings it up to the current Latihan 1/2/3 structure.
    let bank_id = quiz_children
        .iter()
        .find(|c| c.quiz_config.as_ref().is_none_or(|q| q.get("question_pool").is_none()))
        .or_else(|| if quiz_children.len() == 1 { quiz_children.first() } else { None })
        .map(|c| c.id)
        .ok_or_else(|| AppError::UnprocessableEntity("bank_missing", "bab ini belum punya bank soal (Latihan 3) — jalankan Susun Bab dulu".to_string()))?;

    let bab_plan = topic.metadata.as_ref().and_then(|m| m.get("bab_plan"));
    let jenjang = standard.as_ref().map(|s| s.jenjang.clone());
    let level = match &jenjang {
        Some(j) => format!("{} · jenjang {j}", topic.tahap_title),
        None => topic.tahap_title.clone(),
    };
    let jenis_soal = bab_plan
        .and_then(|p| p.get("jenis_soal"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| standard.as_ref().map(|s| s.jenis_soal.clone()))
        .unwrap_or_else(|| "hitungan".to_string());
    let language = standard.as_ref().map(|s| s.bahasa.clone()).unwrap_or_else(|| "id".to_string());

    Ok(ContentTaskContext {
        bab_id,
        topic_id: bab.topic_id,
        topic_title: topic.topic_title,
        bab_title: bab.title.clone(),
        level,
        language,
        jenis_soal,
        siblings,
        position,
        notes: bab_brief(bab_plan, &bab.title),
        article_id,
        bank_id,
    })
}

async fn set_status(pool: &PgPool, task_id: Uuid, status: &str) -> Result<(), AppError> {
    sqlx::query!(r#"update generation_tasks set status = $2, updated_at = now() where id = $1"#, task_id, status).execute(pool).await?;
    Ok(())
}

async fn add_tokens(pool: &PgPool, task_id: Uuid, run_id: Uuid, generate: i64, judge: i64) -> Result<(), AppError> {
    sqlx::query!(r#"update generation_tasks set tokens_generate = tokens_generate + $2, tokens_qa = tokens_qa + $3, updated_at = now() where id = $1"#, task_id, generate, judge).execute(pool).await?;
    sqlx::query!(r#"update generation_runs set tokens_used = tokens_used + $2, updated_at = now() where id = $1"#, run_id, generate + judge).execute(pool).await?;
    Ok(())
}

/// Stops spending before a call that would go over — the run's own
/// token limit, or a role's daily budget from Pengaturan AI. Same
/// mechanism as `runner::budget_problem`, checked against the three
/// roles this kind actually spends on.
async fn budget_problem(pool: &PgPool, run_id: Uuid) -> Result<Option<String>, AppError> {
    let run = sqlx::query!(r#"select tokens_used, token_limit from generation_runs where id = $1"#, run_id).fetch_one(pool).await?;
    if let Some(limit) = run.token_limit {
        if run.tokens_used >= limit {
            return Ok(Some(format!("batas token run tercapai ({} dari {limit})", run.tokens_used)));
        }
    }
    let today = sqlx::query!(
        r#"select coalesce(sum(tokens_generate), 0)::bigint as "generate!", coalesce(sum(tokens_qa), 0)::bigint as "judge!"
           from generation_tasks where kind = 'bab_content' and updated_at >= date_trunc('day', now() at time zone 'Asia/Jakarta') at time zone 'Asia/Jakarta'"#,
    )
    .fetch_one(pool)
    .await?;
    for (role, used) in [(LESSON_ROLE, today.generate), (QUIZ_ROLE, today.generate), (JUDGE_ROLE, today.judge)] {
        let budget = sqlx::query_scalar!(r#"select daily_token_budget from ai_role_settings where role = $1 and enabled"#, role).fetch_optional(pool).await?.flatten();
        if let Some(budget) = budget {
            if used >= budget as i64 {
                return Ok(Some(format!("anggaran token harian peran {role} habis ({used} dari {budget})")));
            }
        }
    }
    Ok(None)
}

async fn pause_run(pool: &PgPool, run_id: Uuid, reason: &str) -> Result<(), AppError> {
    sqlx::query!(r#"update generation_runs set status = 'paused', pause_reason = $2, updated_at = now() where id = $1 and status in ('queued', 'running')"#, run_id, reason).execute(pool).await?;
    Ok(())
}

fn system_ctx(owner: Uuid) -> AuthContext {
    AuthContext { user_id: owner, organization_id: None, role: Some("platform_admin".to_string()) }
}

struct RuleCheck {
    blocking: Vec<String>,
    notes: Vec<String>,
}

/// No AI, no tokens: the same structural checks `generate_bab` already
/// leaves signal for (a bank that never finished filling, a materi with
/// an empty or absurdly short section). Anything here is a `blocker` —
/// the judge isn't asked to rediscover what code already knows.
fn check_rules(outcome: &content::GenerateOutcome) -> RuleCheck {
    let mut blocking = Vec::new();
    let mut notes = Vec::new();
    if outcome.bank_deficit > 0 {
        blocking.push(format!("bank belum lengkap: {} slot masih kosong setelah semua percobaan", outcome.bank_deficit));
    }
    if outcome.lesson_plan.sections.len() < 2 {
        blocking.push(format!("modul hanya {} bagian — terlalu pendek untuk satu bab", outcome.lesson_plan.sections.len()));
    }
    for (i, s) in outcome.lesson_plan.sections.iter().enumerate() {
        if s.content.trim().split_whitespace().count() < 30 {
            blocking.push(format!("bagian {} (\"{}\") terlalu pendek atau kosong", i + 1, s.title));
        }
    }
    if outcome.bank_filled == 0 {
        blocking.push("bank soal kosong".to_string());
    } else {
        notes.push(format!("bank terisi {}/{}", outcome.bank_filled, super::blueprint::BANK_SIZE));
    }
    RuleCheck { blocking, notes }
}

async fn call_judge(jc: &JobContext, owner: Uuid, prompt: String) -> Result<(String, i64), AppError> {
    let resolved = crate::services::ai_settings::resolve(&jc.pool, &jc.config, JUDGE_ROLE).await?;
    let max_tokens = crate::services::ai_provider::resolve_max_tokens(&jc.pool, &resolved.model_id, 32_000).await;
    let request = GenerationRequest {
        model: resolved.model_id.clone(),
        system_prompt: content_qa::system_prompt(),
        user_prompt: prompt,
        temperature: resolved.temperature.unwrap_or(0.1),
        max_tokens,
        image_url: None,
        json_mode: true,
        thinking_budget: Some(2048),
        allow_partial: false,
    };
    let (result, model) = crate::services::ai_provider::generate_with_fallback(jc.text_ai.as_ref(), &resolved, request).await;
    let ai_task_id = Uuid::new_v4();
    match result {
        Ok(resp) => {
            let tokens = resp.tokens_used.unwrap_or(0);
            crate::services::ai_task::insert_done(&jc.pool, ai_task_id, owner, "content_factory_content_qa", "vertex", &model, "bab_content.v1", Some(tokens as i32)).await?;
            Ok((resp.text, tokens as i64))
        }
        Err(e) => {
            let _ = crate::services::ai_task::insert_failed(&jc.pool, ai_task_id, owner, "content_factory_content_qa", "vertex", &model, "bab_content.v1").await;
            Err(AppError::Internal(anyhow::anyhow!("agent_qa gagal: {e}")))
        }
    }
}

/// Runs one `bab_content` task to a resting state.
pub async fn run_bab_content(jc: &JobContext, task_id: Uuid) -> Result<Outcome, AppError> {
    let pool = &jc.pool;
    let task = sqlx::query!(r#"select id, run_id, status, target_id, context from generation_tasks where id = $1"#, task_id).fetch_optional(pool).await?.ok_or(AppError::NotFound("generation_task_not_found"))?;
    if ["applied", "needs_review", "rejected", "cancelled", "benchmarked"].contains(&task.status.as_str()) {
        return Ok(Outcome::Done);
    }
    let run = sqlx::query!(r#"select status, benchmark, options, created_by from generation_runs where id = $1"#, task.run_id).fetch_one(pool).await?;
    match run.status.as_str() {
        "cancelled" => {
            set_status(pool, task.id, "cancelled").await?;
            return Ok(Outcome::Done);
        }
        "paused" => {
            sqlx::query!(r#"update generation_tasks set status = 'queued', job_id = null, updated_at = now() where id = $1"#, task.id).execute(pool).await?;
            return Ok(Outcome::Deferred);
        }
        _ => {}
    }
    if run.benchmark {
        return Err(AppError::UnprocessableEntity(
            "benchmark_not_supported",
            "mode benchmark untuk isi konten belum ada di dashboard — jalankan `cargo run --bin benchmark_bab_content` dari server".to_string(),
        ));
    }
    sqlx::query!(r#"update generation_runs set status = 'running', started_at = coalesce(started_at, now()), updated_at = now() where id = $1 and status = 'queued'"#, task.run_id).execute(pool).await?;
    let owner = run.created_by.ok_or_else(|| AppError::Internal(anyhow::anyhow!("run has no owner")))?;
    let options: RunOptions = serde_json::from_value(run.options).unwrap_or_default();
    let ctx = system_ctx(owner);

    if let Some(reason) = budget_problem(pool, task.run_id).await? {
        pause_run(pool, task.run_id, &reason).await?;
        sqlx::query!(r#"update generation_tasks set status = 'queued', job_id = null, updated_at = now() where id = $1"#, task.id).execute(pool).await?;
        return Ok(Outcome::Deferred);
    }

    let context: ContentTaskContext = match task.context.clone().and_then(|v| serde_json::from_value(v).ok()) {
        Some(c) => c,
        None => {
            let c = load_context(pool, task.target_id).await?;
            sqlx::query!(r#"update generation_tasks set context = $2, prompt_version = 'bab_content.v1', updated_at = now() where id = $1"#, task.id, serde_json::to_value(&c).unwrap_or(Value::Null)).execute(pool).await?;
            c
        }
    };

    set_status(pool, task.id, "generating").await?;
    let lesson_model = crate::services::ai_settings::resolve(pool, &jc.config, LESSON_ROLE).await?.model_id;
    let quiz_model = crate::services::ai_settings::resolve(pool, &jc.config, QUIZ_ROLE).await?.model_id;
    let storage = InMemoryStorage::new();
    let brief = content::BabBrief {
        bab_title: context.bab_title.clone(),
        topic_title: context.topic_title.clone(),
        level: context.level.clone(),
        language: context.language.clone(),
        jenis_soal: context.jenis_soal.clone(),
        siblings: context.siblings.clone(),
        position: context.position,
        notes: context.notes.clone(),
    };
    let items = content::BabItems { section_id: context.bab_id, module_id: context.topic_id, article_id: context.article_id, bank_id: context.bank_id };

    let outcome = content::generate_bab(pool, &jc.config, jc.text_ai.as_ref(), &storage, &lesson_model, &quiz_model, &ctx, &items, &brief).await?;
    add_tokens(pool, task.id, task.run_id, outcome.tokens, 0).await?;

    set_status(pool, task.id, "validating").await?;
    let rules = check_rules(&outcome);
    sqlx::query!(r#"update generation_tasks set validation = $2, updated_at = now() where id = $1"#, task.id, json!({"blocking": rules.blocking, "notes": rules.notes})).execute(pool).await?;

    set_status(pool, task.id, "qa").await?;
    let sample = content::sample_questions(pool, context.bank_id, SAMPLE_SIZE).await?;
    let content_sample = content_qa::ContentSample { bab_title: &context.bab_title, level: &context.level, plan: &outcome.lesson_plan, sample_questions: &sample };
    let prompt = content_qa::score_prompt(&content_sample);
    let (judge_text, judge_tokens) = call_judge(jc, owner, prompt).await?;
    add_tokens(pool, task.id, task.run_id, 0, judge_tokens).await?;
    let score = content_qa::parse_score(&judge_text).map_err(|e| AppError::Internal(anyhow::anyhow!("keluaran penilai tidak terbaca: {e}")))?;
    sqlx::query!(r#"update generation_tasks set qa = $2, updated_at = now() where id = $1"#, task.id, serde_json::to_value(&score).unwrap_or(Value::Null)).execute(pool).await?;

    let verdict = content_qa::decide(&score, options.qa_threshold);
    let passes = rules.blocking.is_empty() && verdict == content_qa::Verdict::Pass;
    let status = if passes { "applied" } else { "needs_review" };
    set_status(pool, task.id, status).await?;
    if passes {
        sqlx::query!(r#"update generation_tasks set applied_at = now() where id = $1"#, task.id).execute(pool).await?;
    }

    super::finish_run_if_idle(pool, task.run_id).await?;
    Ok(Outcome::Done)
}
