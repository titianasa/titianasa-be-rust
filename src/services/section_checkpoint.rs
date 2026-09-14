// The comprehension checkpoint at the end of each Modul Belajar section.
//
// A learner reads a section, answers a small server-side draw from that
// section's checkpoint pool, and only then unlocks the next section.
// Every answer is graded here — the client never holds a key — and a
// wrong answer locks the section again: the learner must re-read it for
// a minimum time before a NEW draw (preferring questions they haven't
// seen) is handed out. When every checkpoint in the article has passed,
// the article counts as complete, which is what opens its Latihan.
//
// Sections are strictly sequential: only the first section that hasn't
// passed is answerable; everything after it reports "locked".

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::lesson_plan::{self, LessonPlan, LessonPlanSection};
use crate::services::quiz_config::{QuestionPool, QuizConfig};
use crate::services::quiz_paper::{self, Paper, PaperInput};
use crate::services::{item_progress, learning_event, module_item, quiz_config_schema, quiz_score, xp};

pub const CHECKPOINT_XP_PER_QUESTION: i32 = 2;
const MIN_REREAD_SECONDS: i64 = 30;
const REREAD_SECONDS_PER_MINUTE: i64 = 15;

#[derive(Debug, Serialize)]
pub struct SectionState {
    pub section_id: String,
    /// "none" (no checkpoint) | "locked" (an earlier checkpoint hasn't
    /// passed) | "open" | "reread" | "passed"
    pub status: &'static str,
    /// The questions to answer — only while "open".
    pub paper: Option<Value>,
    /// While "reread": when a new draw may be requested.
    pub reread_ready_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct CheckpointsResponse {
    pub has_checkpoints: bool,
    pub all_passed: bool,
    pub sections: Vec<SectionState>,
}

#[derive(Debug, Serialize)]
pub struct CheckpointQuestionResult {
    pub question_number: String,
    pub correct: bool,
    pub explanation: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CheckpointSubmitResponse {
    pub passed: bool,
    pub results: Vec<CheckpointQuestionResult>,
    pub all_passed: bool,
    pub xp_awarded: i32,
    pub reread_ready_at: Option<DateTime<Utc>>,
}

struct ProgressRow {
    section_id: String,
    status: String,
    draw: Option<Value>,
    locked_at: Option<DateTime<Utc>>,
}

fn pool_of(section: &LessonPlanSection) -> Option<QuizConfig> {
    section.checkpoint.as_ref().and_then(|raw| quiz_config_schema::parse(raw).ok()).filter(|p| p.all_questions().next().is_some())
}

pub fn reread_seconds(section: &LessonPlanSection) -> i64 {
    (section.minutes.unwrap_or(2) * REREAD_SECONDS_PER_MINUTE).max(MIN_REREAD_SECONDS)
}

/// Access follows the article itself (published, not gated) — the same
/// checks `GET /module-items/{id}` applies — and the plan is read raw so
/// the checkpoint pools are available here even though a learner's own
/// view of the plan has them stripped.
async fn load_plan(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<LessonPlan, AppError> {
    let item = module_item::get_detail(pool, ctx, item_id).await?;
    if item.content_type.as_deref() != Some("article") {
        return Err(AppError::UnprocessableEntity("not_an_article", "checkpoint hanya ada di Modul Belajar".to_string()));
    }
    let raw = sqlx::query_scalar!(r#"select lesson_plan from module_items where id = $1"#, item_id).fetch_one(pool).await?;
    match raw {
        Some(raw) => lesson_plan::parse(&raw),
        None => Ok(LessonPlan::default()),
    }
}

async fn load_rows(pool: &PgPool, user_id: Uuid, item_id: Uuid) -> Result<HashMap<String, ProgressRow>, AppError> {
    let rows = sqlx::query_as!(
        ProgressRow,
        r#"select section_id, status, draw, locked_at from section_checkpoint_progress where user_id = $1 and item_id = $2"#,
        user_id,
        item_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.section_id.clone(), r)).collect())
}

fn new_draw(item_id: Uuid, pool_config: &QuizConfig, previous: Option<&Value>) -> Paper {
    let recently_seen: HashSet<Uuid> = previous
        .and_then(|v| serde_json::from_value::<Paper>(v.clone()).ok())
        .map(|p| p.questions.into_iter().map(|q| q.uid).collect())
        .unwrap_or_default();
    let policy = QuizConfig { question_pool: Some(QuestionPool { source_item_id: None, draw_count: lesson_plan::CHECKPOINT_DRAW }), ..Default::default() };
    quiz_paper::build(PaperInput { source_item_id: item_id, source: pool_config, policy: &policy, recently_seen: &recently_seen, seed: quiz_paper::seed_from(Uuid::new_v4()) })
}

async fn store_draw(pool: &PgPool, user_id: Uuid, item_id: Uuid, section_id: &str, paper: &Paper) -> Result<(), AppError> {
    let draw = serde_json::to_value(paper).map_err(|e| AppError::Internal(e.into()))?;
    sqlx::query!(
        r#"insert into section_checkpoint_progress (user_id, item_id, section_id, status, draw)
           values ($1, $2, $3, 'open', $4)
           on conflict (user_id, item_id, section_id) do update set status = 'open', draw = excluded.draw, locked_at = null, updated_at = now()"#,
        user_id,
        item_id,
        section_id,
        draw,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn all_passed(plan: &LessonPlan, rows: &HashMap<String, ProgressRow>) -> bool {
    plan.sections.iter().filter(|s| pool_of(s).is_some()).all(|s| rows.get(&s.id).is_some_and(|r| r.status == "passed"))
}

/// Whether an article's checkpoints (if it has any) let it be completed.
pub async fn completion_allowed(pool: &PgPool, user_id: Uuid, item_id: Uuid) -> Result<bool, AppError> {
    let raw = sqlx::query_scalar!(r#"select lesson_plan from module_items where id = $1"#, item_id).fetch_optional(pool).await?.flatten();
    let Some(plan) = raw.and_then(|r| lesson_plan::parse(&r).ok()) else { return Ok(true) };
    if !plan.sections.iter().any(|s| pool_of(s).is_some()) {
        return Ok(true);
    }
    Ok(all_passed(&plan, &load_rows(pool, user_id, item_id).await?))
}

// GET /module-items/{id}/checkpoints
pub async fn state(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<CheckpointsResponse, AppError> {
    let plan = load_plan(pool, ctx, item_id).await?;
    let mut rows = load_rows(pool, ctx.user_id, item_id).await?;
    let mut sections = Vec::new();
    let mut blocked = false;

    for section in &plan.sections {
        let Some(pool_config) = pool_of(section) else {
            sections.push(SectionState { section_id: section.id.clone(), status: if blocked { "locked" } else { "none" }, paper: None, reread_ready_at: None });
            continue;
        };
        if blocked {
            sections.push(SectionState { section_id: section.id.clone(), status: "locked", paper: None, reread_ready_at: None });
            continue;
        }
        let row = rows.get(&section.id);
        let state = match row.map(|r| r.status.as_str()) {
            Some("passed") => SectionState { section_id: section.id.clone(), status: "passed", paper: None, reread_ready_at: None },
            Some("reread") => {
                blocked = true;
                let ready = row.and_then(|r| r.locked_at).map(|t| t + chrono::Duration::seconds(reread_seconds(section)));
                SectionState { section_id: section.id.clone(), status: "reread", paper: None, reread_ready_at: ready }
            }
            _ => {
                blocked = true;
                let existing = row.and_then(|r| r.draw.clone()).and_then(|v| serde_json::from_value::<Paper>(v).ok());
                let paper = match existing {
                    Some(paper) => paper,
                    None => {
                        let paper = new_draw(item_id, &pool_config, None);
                        store_draw(pool, ctx.user_id, item_id, &section.id, &paper).await?;
                        paper
                    }
                };
                SectionState { section_id: section.id.clone(), status: "open", paper: Some(paper.config), reread_ready_at: None }
            }
        };
        sections.push(state);
    }

    let has_checkpoints = plan.sections.iter().any(|s| pool_of(s).is_some());
    rows.retain(|_, r| r.status == "passed");
    Ok(CheckpointsResponse { has_checkpoints, all_passed: all_passed(&plan, &rows), sections })
}

// ── Preview (/belajar/{id}/preview) ─────────────────────────────────
//
// An author checking a Modul Belajar answers its checkpoints the way a
// learner would — the same draw, the same shuffled and re-lettered
// choices, the same grader — but nothing is stored: no progress row, no
// lock, no XP, no learning event. The draw travels to the client and
// back instead of living in `section_checkpoint_progress`; that is safe
// only because whoever may preview can already read the answer keys.

#[derive(Debug, Serialize)]
pub struct CheckpointPreviewDraw {
    /// What to render — same shape as `SectionState.paper`.
    pub paper: Value,
    /// Hand this back to `preview_grade` with the answers.
    pub draw: Paper,
}

async fn preview_pool(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, section_id: &str) -> Result<QuizConfig, AppError> {
    if !crate::services::assessment::can_preview(pool, ctx, item_id).await? {
        return Err(AppError::ForbiddenWithCode("preview_not_allowed"));
    }
    let plan = load_plan(pool, ctx, item_id).await?;
    let (_, _, pool_config) = find_section(&plan, section_id)?;
    Ok(pool_config)
}

// POST /module-items/{id}/sections/{section_id}/checkpoint/preview
pub async fn preview_draw(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, section_id: &str, previous: Option<&Value>) -> Result<CheckpointPreviewDraw, AppError> {
    let pool_config = preview_pool(pool, ctx, item_id, section_id).await?;
    let draw = new_draw(item_id, &pool_config, previous);
    Ok(CheckpointPreviewDraw { paper: draw.config.clone(), draw })
}

// POST /module-items/{id}/sections/{section_id}/checkpoint/preview/grade
pub async fn preview_grade(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, section_id: &str, draw: &Paper, answers: &HashMap<String, Value>) -> Result<CheckpointSubmitResponse, AppError> {
    let pool_config = preview_pool(pool, ctx, item_id, section_id).await?;
    let translated = quiz_paper::translate_answers(draw, answers);
    let mut results = Vec::new();
    for paper_q in &draw.questions {
        // Graded against the pool as it is NOW, never against anything
        // the client sent back but the uids and letter mapping.
        let Some((group, question)) = pool_config.all_questions().find(|(_, q)| q.uid == Some(paper_q.uid)) else { continue };
        let score = quiz_score::score_question(&group.r#type, question, &translated, Some(group));
        results.push(CheckpointQuestionResult { question_number: paper_q.shown_key.clone(), correct: score.is_correct, explanation: question.explanation.as_deref().map(|text| quiz_paper::explanation_as_shown(paper_q, text)) });
    }
    let passed = !results.is_empty() && results.iter().all(|r| r.correct);
    Ok(CheckpointSubmitResponse { passed, results, all_passed: false, xp_awarded: 0, reread_ready_at: None })
}

fn find_section<'a>(plan: &'a LessonPlan, section_id: &str) -> Result<(usize, &'a LessonPlanSection, QuizConfig), AppError> {
    let (index, section) = plan.sections.iter().enumerate().find(|(_, s)| s.id == section_id).ok_or(AppError::NotFound("section_not_found"))?;
    let pool_config = pool_of(section).ok_or(AppError::NotFound("checkpoint_not_found"))?;
    Ok((index, section, pool_config))
}

fn earlier_all_passed(plan: &LessonPlan, index: usize, rows: &HashMap<String, ProgressRow>) -> bool {
    plan.sections[..index].iter().filter(|s| pool_of(s).is_some()).all(|s| rows.get(&s.id).is_some_and(|r| r.status == "passed"))
}

// POST /module-items/{id}/sections/{section_id}/checkpoint
pub async fn submit(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, section_id: &str, answers: &HashMap<String, Value>) -> Result<CheckpointSubmitResponse, AppError> {
    let plan = load_plan(pool, ctx, item_id).await?;
    let (index, section, pool_config) = find_section(&plan, section_id)?;
    let rows = load_rows(pool, ctx.user_id, item_id).await?;
    if !earlier_all_passed(&plan, index, &rows) {
        return Err(AppError::UnprocessableEntity("checkpoint_locked", "selesaikan checkpoint bagian sebelumnya dulu".to_string()));
    }
    let row = rows.get(section_id);
    let paper = match row {
        Some(r) if r.status == "open" => r.draw.clone().and_then(|v| serde_json::from_value::<Paper>(v).ok()),
        Some(r) if r.status == "passed" => return Err(AppError::Conflict("checkpoint_already_passed")),
        Some(r) if r.status == "reread" => return Err(AppError::UnprocessableEntity("checkpoint_needs_reread", "baca ulang bagian ini dulu sebelum mencoba lagi".to_string())),
        _ => None,
    }
    .ok_or(AppError::Conflict("checkpoint_not_open"))?;

    let translated = quiz_paper::translate_answers(&paper, answers);
    let mut results = Vec::new();
    let mut correct_uids = Vec::new();
    for paper_q in &paper.questions {
        let Some((group, question)) = pool_config.all_questions().find(|(_, q)| q.uid == Some(paper_q.uid)) else { continue };
        let score = quiz_score::score_question(&group.r#type, question, &translated, Some(group));
        if score.is_correct {
            correct_uids.push(paper_q.uid);
        }
        let mut event = learning_event::NewLearningEvent::server(
            "question_answered",
            "question",
            paper_q.uid,
            serde_json::json!({"correct": score.is_correct, "difficulty": question.taxonomy.as_ref().and_then(|t| t.difficulty), "answer": translated.get(&paper_q.original_key), "checkpoint": true}),
            "self_learning",
        );
        event.module_item_id = Some(item_id);
        event.content_uid = Some(paper_q.uid.to_string());
        learning_event::record(pool, ctx.user_id, learning_event::EventChannel::Server, event).await?;
        results.push(CheckpointQuestionResult { question_number: paper_q.shown_key.clone(), correct: score.is_correct, explanation: question.explanation.as_deref().map(|text| quiz_paper::explanation_as_shown(paper_q, text)) });
    }
    let passed = !results.is_empty() && results.iter().all(|r| r.correct);

    // Checkpoint answers count toward the heatmap too, placed under the
    // section they check (migrations/0058).
    let facts = paper
        .questions
        .iter()
        .filter_map(|paper_q| {
            let (group, question) = pool_config.all_questions().find(|(_, q)| q.uid == Some(paper_q.uid))?;
            let correct = results.iter().find(|r| r.question_number == paper_q.shown_key).map(|r| r.correct);
            let (difficulty, bloom) = crate::services::answer_facts::taxonomy_labels(question);
            Some(crate::services::answer_facts::FactInput {
                question_uid: paper_q.uid,
                content_item_id: item_id,
                source_section_id: Some(section_id.to_string()),
                subtype: group.r#type.clone(),
                difficulty,
                bloom,
                correct,
                points_earned: if correct == Some(true) { 1.0 } else { 0.0 },
                points_max: 1.0,
                content_version: None,
            })
        })
        .collect();
    crate::services::answer_facts::record(pool, ctx.user_id, item_id, "checkpoint", None, facts).await?;

    let mut xp_awarded = 0;
    let mut reread_ready_at = None;
    if passed {
        sqlx::query!(
            r#"update section_checkpoint_progress set status = 'passed', passed_at = now(), attempts = attempts + 1, updated_at = now()
               where user_id = $1 and item_id = $2 and section_id = $3"#,
            ctx.user_id,
            item_id,
            section_id,
        )
        .execute(pool)
        .await?;
        // A question answered right pays once, ever — failing and
        // re-answering the same question later doesn't pay it again.
        for uid in correct_uids {
            if xp::award_xp(pool, ctx.user_id, CHECKPOINT_XP_PER_QUESTION, "checkpoint_correct", Some(&format!("checkpoint_q:{}:{item_id}:{uid}", ctx.user_id)), None).await? {
                xp_awarded += CHECKPOINT_XP_PER_QUESTION;
            }
        }
    } else {
        let locked = sqlx::query_scalar!(
            r#"update section_checkpoint_progress set status = 'reread', locked_at = now(), attempts = attempts + 1, updated_at = now()
               where user_id = $1 and item_id = $2 and section_id = $3 returning locked_at as "locked_at!""#,
            ctx.user_id,
            item_id,
            section_id,
        )
        .fetch_one(pool)
        .await?;
        reread_ready_at = Some(locked + chrono::Duration::seconds(reread_seconds(section)));
    }

    let rows = load_rows(pool, ctx.user_id, item_id).await?;
    let done = all_passed(&plan, &rows);
    if passed && done {
        item_progress::record_completion(pool, ctx.user_id, item_id, None, "self_learning").await?;
    }
    Ok(CheckpointSubmitResponse { passed, results, all_passed: done, xp_awarded, reread_ready_at })
}

// POST /module-items/{id}/sections/{section_id}/reread
pub async fn reread(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, section_id: &str) -> Result<CheckpointsResponse, AppError> {
    let plan = load_plan(pool, ctx, item_id).await?;
    let (_, section, pool_config) = find_section(&plan, section_id)?;
    let rows = load_rows(pool, ctx.user_id, item_id).await?;
    let Some(row) = rows.get(section_id).filter(|r| r.status == "reread") else {
        return Err(AppError::Conflict("checkpoint_not_locked"));
    };
    let ready = row.locked_at.unwrap_or_else(Utc::now) + chrono::Duration::seconds(reread_seconds(section));
    let remaining = (ready - Utc::now()).num_seconds();
    if remaining > 0 {
        return Err(AppError::UnprocessableEntity("reread_too_soon", format!("baca ulang bagian ini dulu — coba lagi dalam {remaining} detik")));
    }
    let paper = new_draw(item_id, &pool_config, row.draw.as_ref());
    store_draw(pool, ctx.user_id, item_id, section_id, &paper).await?;
    state(pool, ctx, item_id).await
}
