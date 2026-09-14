// One task through the factory: generate → validate → QA → repair what
// failed → apply. Runs inside a `content_generation` job on titian-worker.
//
// Progress is written to the task row after every step, so a job that
// dies halfway (worker restart, provider outage) resumes where it was:
// topics already applied stay applied, the next round only regenerates
// what is still failing.

use std::collections::HashMap;

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::plan::{self, DomainContext, TopicDraft, TopicStatus};
use super::qa::{self, TopicScore, Verdict};
use super::validate::{self, Issue};
use super::{apply, RunOptions};
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::{generate_with_fallback, resolve_max_tokens, GenerationRequest};
use crate::services::ai_settings;
use crate::services::job_queue::JobContext;

const GENERATOR_ROLE: &str = "curriculum_planning";
const JUDGE_ROLE: &str = "agent_qa";
const EXEMPLARS: i64 = 2;

struct TaskRow {
    id: Uuid,
    run_id: Uuid,
    status: String,
    target_id: Uuid,
    attempt: i32,
    draft: Option<Value>,
    context: Option<Value>,
    history: Value,
    scope_topics: Option<Vec<Uuid>>,
}

#[derive(Debug)]
pub enum Outcome {
    Done,
    /// Put back to `queued` without spending an attempt (run paused).
    Deferred,
}

async fn set_status(pool: &PgPool, task_id: Uuid, status: &str) -> Result<(), AppError> {
    sqlx::query!(r#"update generation_tasks set status = $2, updated_at = now() where id = $1"#, task_id, status).execute(pool).await?;
    Ok(())
}

async fn save_progress(pool: &PgPool, task_id: Uuid, drafts: &[TopicDraft], validation: &[Issue], scores: &HashMap<Uuid, TopicScore>, history: &Value, attempt: i32) -> Result<(), AppError> {
    let scores: Vec<&TopicScore> = scores.values().collect();
    sqlx::query!(
        r#"update generation_tasks set draft = $2, validation = $3, qa = $4, history = $5, attempt = $6, updated_at = now() where id = $1"#,
        task_id,
        serde_json::to_value(drafts).unwrap_or(Value::Null),
        serde_json::to_value(validation).unwrap_or(Value::Null),
        serde_json::to_value(scores).unwrap_or(Value::Null),
        history,
        attempt,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn add_tokens(pool: &PgPool, task_id: Uuid, run_id: Uuid, generate: i64, judge: i64) -> Result<(), AppError> {
    sqlx::query!(r#"update generation_tasks set tokens_generate = tokens_generate + $2, tokens_qa = tokens_qa + $3, updated_at = now() where id = $1"#, task_id, generate, judge).execute(pool).await?;
    sqlx::query!(r#"update generation_runs set tokens_used = tokens_used + $2, updated_at = now() where id = $1"#, run_id, generate + judge).execute(pool).await?;
    Ok(())
}

/// Stops spending before a call that would go over: the run's own token
/// limit, or a role's daily budget from Pengaturan AI (the column existed
/// before this but nothing ever read it).
async fn budget_problem(pool: &PgPool, run_id: Uuid) -> Result<Option<String>, AppError> {
    let run = sqlx::query!(r#"select tokens_used, token_limit from generation_runs where id = $1"#, run_id).fetch_one(pool).await?;
    if let Some(limit) = run.token_limit {
        if run.tokens_used >= limit {
            return Ok(Some(format!("batas token run tercapai ({} dari {limit})", run.tokens_used)));
        }
    }
    let today = sqlx::query!(
        r#"select coalesce(sum(tokens_generate), 0)::bigint as "generate!", coalesce(sum(tokens_qa), 0)::bigint as "judge!"
           from generation_tasks where updated_at >= date_trunc('day', now() at time zone 'Asia/Jakarta') at time zone 'Asia/Jakarta'"#,
    )
    .fetch_one(pool)
    .await?;
    for (role, used) in [(GENERATOR_ROLE, today.generate), (JUDGE_ROLE, today.judge)] {
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

struct Call {
    text: String,
    tokens: i64,
    model: String,
}

async fn call(jc: &JobContext, owner: Uuid, role: &str, system_prompt: String, user_prompt: String, thinking: i64, temperature_default: f64, task_type: &str) -> Result<Call, AppError> {
    let resolved = ai_settings::resolve(&jc.pool, &jc.config, role).await?;
    let max_tokens = resolve_max_tokens(&jc.pool, &resolved.model_id, resolved.max_tokens.map(i64::from).unwrap_or(32_000)).await;
    let request = GenerationRequest {
        model: resolved.model_id.clone(),
        system_prompt,
        user_prompt,
        temperature: resolved.temperature.unwrap_or(temperature_default),
        max_tokens,
        image_url: None,
        json_mode: true,
        thinking_budget: Some(thinking),
        allow_partial: false,
    };
    let (result, model) = generate_with_fallback(jc.text_ai.as_ref(), &resolved, request).await;
    let ai_task_id = Uuid::new_v4();
    match result {
        Ok(resp) => {
            let tokens = resp.tokens_used.unwrap_or(0);
            crate::services::ai_task::insert_done(&jc.pool, ai_task_id, owner, task_type, "vertex", &model, plan::PROMPT_VERSION, Some(tokens as i32)).await?;
            Ok(Call { text: resp.text, tokens, model })
        }
        Err(e) => {
            let _ = crate::services::ai_task::insert_failed(&jc.pool, ai_task_id, owner, task_type, "vertex", &model, plan::PROMPT_VERSION).await;
            Err(AppError::Internal(anyhow::anyhow!("{role} gagal: {e}")))
        }
    }
}

fn system_ctx(owner: Uuid) -> AuthContext {
    AuthContext { user_id: owner, organization_id: None, role: Some("platform_admin".to_string()) }
}

async fn load_task(pool: &PgPool, task_id: Uuid) -> Result<TaskRow, AppError> {
    let r = sqlx::query!(
        r#"select t.id, t.run_id, t.status, t.target_id, t.attempt, t.draft, t.context, t.history,
                  (select array_agg(value::text::uuid) from jsonb_array_elements_text(
                     (select d->'topic_ids' from jsonb_array_elements(r.scope->'domains') d where (d->>'domain_id')::uuid = t.target_id limit 1)
                   ) value) as scope_topics
           from generation_tasks t join generation_runs r on r.id = t.run_id where t.id = $1"#,
        task_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("generation_task_not_found"))?;
    Ok(TaskRow { id: r.id, run_id: r.run_id, status: r.status, target_id: r.target_id, attempt: r.attempt, draft: r.draft, context: r.context, history: r.history, scope_topics: r.scope_topics })
}

/// Runs one `bab_plan` task to a resting state.
pub async fn run_bab_plan(jc: &JobContext, task_id: Uuid) -> Result<Outcome, AppError> {
    let pool = &jc.pool;
    let task = load_task(pool, task_id).await?;
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
    sqlx::query!(r#"update generation_runs set status = 'running', started_at = coalesce(started_at, now()), updated_at = now() where id = $1 and status = 'queued'"#, task.run_id).execute(pool).await?;
    let owner = run.created_by.ok_or_else(|| AppError::Internal(anyhow::anyhow!("run has no owner")))?;
    let options: RunOptions = serde_json::from_value(run.options).unwrap_or_default();
    let ctx = system_ctx(owner);

    // Context: fixed at the first round so every round (and a resumed
    // job) plans against the same picture.
    let domain: DomainContext = match task.context.clone().and_then(|v| serde_json::from_value(v).ok()) {
        Some(d) => d,
        None => {
            let d = plan::load_context(pool, task.target_id, task.scope_topics.as_deref()).await?;
            sqlx::query!(r#"update generation_tasks set context = $2, prompt_version = $3, updated_at = now() where id = $1"#, task.id, serde_json::to_value(&d).unwrap_or(Value::Null), plan::PROMPT_VERSION).execute(pool).await?;
            d
        }
    };
    if domain.target_topic_ids.is_empty() {
        sqlx::query!(r#"update generation_tasks set status = 'applied', error = 'semua topik di domain ini sudah punya bab', updated_at = now() where id = $1"#, task.id).execute(pool).await?;
        return Ok(Outcome::Done);
    }
    let exemplars = plan::load_exemplars(pool, &domain, EXEMPLARS).await?;
    let targets = domain.targets();

    let mut drafts: Vec<TopicDraft> = task.draft.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let mut history = task.history;
    let mut scores: HashMap<Uuid, TopicScore> = HashMap::new();
    let mut issues: Vec<Issue> = Vec::new();
    let mut attempt = task.attempt;
    let max_rounds = options.max_repairs + 1;

    while attempt < max_rounds {
        if let Some(reason) = budget_problem(pool, task.run_id).await? {
            pause_run(pool, task.run_id, &reason).await?;
            sqlx::query!(r#"update generation_tasks set status = 'queued', job_id = null, updated_at = now() where id = $1"#, task.id).execute(pool).await?;
            return Ok(Outcome::Deferred);
        }
        attempt += 1;

        // ── generate: everything on round 1, only failing topics after ──
        let pending: Vec<&plan::TopicContext> = targets.iter().copied().filter(|t| !drafts.iter().any(|d| d.topic_id == t.topic_id && matches!(d.status, TopicStatus::Passed | TopicStatus::Applied))).collect();
        if pending.is_empty() {
            break;
        }
        set_status(pool, task.id, "generating").await?;
        let is_repair = drafts.iter().any(|d| pending.iter().any(|p| p.topic_id == d.topic_id));
        let (numbering, prompt) = if is_repair {
            let repair: Vec<(usize, &plan::TopicContext, Option<&TopicDraft>, Vec<String>)> = pending
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mut notes: Vec<String> = issues.iter().filter(|x| x.topic_id == t.topic_id && x.severity >= validate::Severity::Major).map(|x| x.message.clone()).collect();
                    if let Some(s) = scores.get(&t.topic_id) {
                        notes.extend(qa::repair_notes(s, options.qa_threshold));
                    }
                    (i + 1, *t, drafts.iter().find(|d| d.topic_id == t.topic_id), notes)
                })
                .collect();
            (pending.clone(), plan::user_prompt(&domain, &exemplars, &repair))
        } else {
            (pending.clone(), plan::user_prompt(&domain, &exemplars, &[]))
        };
        let generated = call(jc, owner, GENERATOR_ROLE, plan::system_prompt(), prompt, 2048, 0.4, "content_factory_bab_plan").await?;
        add_tokens(pool, task.id, task.run_id, generated.tokens, 0).await?;
        let fresh = match plan::parse(&generated.text, &numbering) {
            Ok(d) => d,
            Err(e) => {
                history_push(&mut history, attempt, "parse_error", json!({"error": e, "model": generated.model}));
                save_progress(pool, task.id, &drafts, &issues, &scores, &history, attempt).await?;
                continue;
            }
        };
        for d in fresh {
            drafts.retain(|x| x.topic_id != d.topic_id);
            drafts.push(d);
        }
        drafts.sort_by_key(|d| targets.iter().position(|t| t.topic_id == d.topic_id));

        // ── validate (rules) ──
        set_status(pool, task.id, "validating").await?;
        issues = validate::validate(&targets, &drafts, &domain.standard.jenjang);
        let broken = validate::needs_repair(&issues);
        for d in drafts.iter_mut().filter(|d| d.status == TopicStatus::Pending || d.status == TopicStatus::NeedsReview) {
            d.status = TopicStatus::Pending;
        }

        // ── QA (judge) on the topics that passed the rules ──
        let to_judge: Vec<&TopicDraft> = drafts.iter().filter(|d| d.status == TopicStatus::Pending && !broken.contains(&d.topic_id)).collect();
        if !to_judge.is_empty() && !run.benchmark {
            set_status(pool, task.id, "qa").await?;
            let (prompt, order) = qa::score_prompt(&domain, &to_judge);
            let judged = call(jc, owner, JUDGE_ROLE, qa::system_prompt(), prompt, 2048, 0.1, "content_factory_qa").await?;
            add_tokens(pool, task.id, task.run_id, 0, judged.tokens).await?;
            match qa::parse_scores(&judged.text, &order) {
                Ok(list) => {
                    for s in list {
                        scores.insert(s.topic_id, s);
                    }
                }
                Err(e) => history_push(&mut history, attempt, "qa_parse_error", json!({"error": e})),
            }
            for d in drafts.iter_mut().filter(|d| d.status == TopicStatus::Pending && !broken.contains(&d.topic_id)) {
                if let Some(s) = scores.get(&d.topic_id) {
                    if qa::decide(s, options.qa_threshold) == Verdict::Pass {
                        d.status = TopicStatus::Passed;
                    }
                }
            }
        } else if run.benchmark {
            for d in drafts.iter_mut().filter(|d| !broken.contains(&d.topic_id)) {
                d.status = TopicStatus::Passed;
            }
        }

        // ── apply what passed (never in a benchmark) ──
        if !run.benchmark {
            for i in 0..drafts.len() {
                if drafts[i].status != TopicStatus::Passed {
                    continue;
                }
                let average = scores.get(&drafts[i].topic_id).map(TopicScore::average);
                apply::apply_topic(pool, &ctx, &domain, &drafts[i], task.id, average, "gemini").await?;
                drafts[i].status = TopicStatus::Applied;
            }
        }

        let failing = drafts.iter().filter(|d| d.status == TopicStatus::Pending).count() + targets.iter().filter(|t| !drafts.iter().any(|d| d.topic_id == t.topic_id)).count();
        history_push(
            &mut history,
            attempt,
            "round",
            json!({"model": generated.model, "rule_issues": issues.len(), "rule_broken": broken.len(), "judged": scores.len(), "still_failing": failing}),
        );
        save_progress(pool, task.id, &drafts, &issues, &scores, &history, attempt).await?;
        if failing == 0 {
            break;
        }
    }

    if run.benchmark {
        super::benchmark::judge_task(jc, owner, task.id, &domain, &drafts).await?;
        set_status(pool, task.id, "benchmarked").await?;
    } else {
        for d in drafts.iter_mut().filter(|d| d.status == TopicStatus::Pending) {
            d.status = TopicStatus::NeedsReview;
        }
        let review = drafts.iter().any(|d| d.status == TopicStatus::NeedsReview) || targets.iter().any(|t| !drafts.iter().any(|d| d.topic_id == t.topic_id));
        save_progress(pool, task.id, &drafts, &issues, &scores, &history, attempt).await?;
        set_status(pool, task.id, if review { "needs_review" } else { "applied" }).await?;
        if !review {
            sqlx::query!(r#"update generation_tasks set applied_at = now() where id = $1"#, task.id).execute(pool).await?;
        }
    }
    super::finish_run_if_idle(pool, task.run_id).await?;
    Ok(Outcome::Done)
}

fn history_push(history: &mut Value, attempt: i32, kind: &str, detail: Value) {
    if !history.is_array() {
        *history = json!([]);
    }
    history.as_array_mut().expect("array").push(json!({"attempt": attempt, "kind": kind, "at": chrono::Utc::now(), "detail": detail}));
}
