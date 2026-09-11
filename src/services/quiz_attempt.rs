use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::AIProvider;
use crate::services::drive_permissions::DriveResource;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::storage::AssetStorage;
use crate::services::quiz_config::{value_to_key, QuizConfig};
use crate::services::{ai_speaking_evaluation, ai_writing_evaluation, assessment, evaluation, quiz_config_schema, quiz_score, quiz_subtype};

// Phase 37 — submission/grading for a `quiz`-type module item, reading
// question_groups straight out of module_items.quiz_config instead of
// the assessment_questions/questions tables assessment::submit_attempt
// uses. Deliberately a separate, simpler pipeline: quiz_config groups
// carry no concept_id links, so there is no mastery/FRSS integration
// here (that Titian-specific pedagogy layer stays scoped to the
// concept-linked `questions` bank) — XP/streak/achievement/daily
// mission hooks still fire, composed by the handler exactly like
// assessment::submit_attempt's.

// Generic rubric for Manual-mode subtypes reviewed by a teacher — a
// single "quality" criterion, since there's no fixed per-subtype rubric
// shape the way writing/speaking have their own 4-5 criteria.
const MANUAL_REVIEW_RUBRIC_ID: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00f3);

async fn ensure_manual_review_rubric(pool: &PgPool) -> Result<evaluation::Rubric, AppError> {
    let criteria = serde_json::json!([{"key": "quality", "label": "Kualitas jawaban"}]);
    evaluation::ensure_rubric(pool, MANUAL_REVIEW_RUBRIC_ID, "Manual Review v1", criteria).await
}

/// One row per QUESTION, not per group — a group is a shared context
/// (a passage, a recording) that many questions draw on, so grading it
/// as a single unit would collapse "8 of 10 right" into one boolean.
#[derive(Debug, serde::Serialize)]
pub struct QuizQuestionResult {
    pub group_id: String,
    /// The question's `number`, stringified — the key the learner's
    /// answer map uses.
    pub question_number: String,
    pub subtype: String,
    pub grading_mode: &'static str,
    pub correct: Option<bool>,
    /// 0-100, only set for an AiRubric question whose evaluation
    /// succeeded — the full per-criterion breakdown/feedback stays in
    /// `evaluations`/`feedback` (not duplicated into this response).
    pub score: Option<f64>,
    pub pending_review: bool,
    /// Multi-mark questions ("choose TWO") are worth one point per slot
    /// and award partial credit, so these are not always 0 or 1.
    pub points_earned: f64,
    pub points_max: f64,
}

#[derive(Debug, serde::Serialize)]
pub struct QuizSubmitResponse {
    pub attempt_id: Uuid,
    pub status: String,
    pub score: Option<f64>,
    pub results: Vec<QuizQuestionResult>,
}

fn grading_mode_label(mode: quiz_subtype::GradingMode) -> &'static str {
    match mode {
        quiz_subtype::GradingMode::Auto => "auto",
        quiz_subtype::GradingMode::AiRubric => "ai_rubric",
        quiz_subtype::GradingMode::Manual => "manual",
        quiz_subtype::GradingMode::SelfCheck => "self_check",
    }
}

async fn read_audio_bytes(pool: &PgPool, storage: &dyn AssetStorage, ctx: &AuthContext, audio_asset_id: Uuid) -> Result<Option<(Vec<u8>, String)>, AppError> {
    let access = crate::services::drive_permissions::resolve_access(pool, ctx, DriveResource::Asset, audio_asset_id).await?;
    if access.is_none() {
        return Ok(None);
    }
    let Some(asset) = crate::services::asset::find_by_id(pool, audio_asset_id).await? else { return Ok(None) };
    match storage.get(&asset.id.to_string()).await {
        Ok(object) => Ok(Some((object.bytes, object.content_type))),
        Err(e) => {
            tracing::warn!(error = ?e, "failed to read audio asset for quiz ai_rubric evaluation");
            Ok(None)
        }
    }
}

// POST /attempts/{id}/submit — quiz branch. `answers` is keyed by
// QUESTION NUMBER (stringified), matching the key the authoring side
// and every renderer use. Value shape follows the subtype's comparison
// family:
//   - auto subtypes: a bare string ("B", "10 years") or, for set-answer
//     subtypes, an array of strings — parelabs' QuizAnswerState shape.
//   - essay-style: a bare string, or {"text": "..."}.
//   - voice/video/file: {"audio_asset_id": "..."} (or "asset_id").
fn answer_text(value: &serde_json::Value) -> &str {
    match value {
        serde_json::Value::String(s) => s,
        serde_json::Value::Object(_) => value.get("text").and_then(|v| v.as_str()).unwrap_or_default(),
        _ => "",
    }
}

fn answer_asset_id(value: &serde_json::Value) -> Option<Uuid> {
    let raw = value
        .get("audio_asset_id")
        .or_else(|| value.get("asset_id"))
        .and_then(|v| v.as_str())
        .or_else(|| value.as_str())?;
    Uuid::parse_str(raw).ok()
}

pub async fn submit_quiz_attempt(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    ctx: &AuthContext,
    attempt_id: Uuid,
    answers: &HashMap<String, serde_json::Value>,
) -> Result<QuizSubmitResponse, AppError> {
    require_permission(ctx, Resource::Attempt, Action::Submit)?;
    let attempt = assessment::load_submittable_attempt(pool, ctx, attempt_id).await?;
    let item_id = attempt.item_id.ok_or_else(|| AppError::Internal(anyhow::anyhow!("quiz attempt has no item_id")))?;
    // A proctored quiz only accepts answers from a sitting that went
    // through its preflight.
    let proctor_session = crate::services::item_proctor::session_for_submit(pool, ctx.user_id, item_id).await?;

    let raw_config = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, item_id)
        .fetch_optional(pool)
        .await?
        .flatten()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("quiz item {} has no quiz_config", item_id)))?;
    let quiz: QuizConfig = quiz_config_schema::parse(&raw_config)?;

    let mut points_possible = 0.0f64;
    let mut points_earned = 0.0f64;
    let mut has_pending = false;
    let mut results = Vec::new();
    let mut answers_snapshot = serde_json::Map::new();

    // quiz_score wants the whole answer map; only the keys it reads are
    // ever touched, so passing it through wholesale is safe.
    let answer_state: quiz_score::AnswerState = answers.clone();

    for group in &quiz.question_groups {
        let subtype = group.r#type.as_str();
        let Some(info) = quiz_subtype::find(subtype) else { continue };

        for question in &group.questions {
            let key = question.key();
            let submitted = answers.get(&key).cloned().unwrap_or(serde_json::Value::Null);
            answers_snapshot.insert(key.clone(), submitted.clone());

            let (correct, ai_score, pending_review, earned, max) = match info.grading_mode {
                quiz_subtype::GradingMode::Auto => {
                    let score = quiz_score::score_question(subtype, question, &answer_state, Some(group));
                    (Some(score.is_correct), None, false, score.points_earned as f64, score.points_max as f64)
                }
                quiz_subtype::GradingMode::AiRubric => {
                    let score = evaluate_with_rubric(pool, config, ai, storage, ctx, attempt_id, subtype, &submitted).await?;
                    // An un-scored rubric question (empty submission, or
                    // the provider failed) contributes 0 of 1 rather than
                    // silently shrinking the denominator.
                    (None, score, false, score.map(|s| s / 100.0).unwrap_or(0.0), 1.0)
                }
                quiz_subtype::GradingMode::Manual => {
                    has_pending = true;
                    // Not counted either way yet — a teacher's score folds
                    // in later via grade_manual_group.
                    (None, None, true, 0.0, 0.0)
                }
                quiz_subtype::GradingMode::SelfCheck => (None, None, false, 0.0, 0.0),
            };

            points_possible += max;
            points_earned += earned;

            results.push(QuizQuestionResult {
                group_id: group.group_id.clone(),
                question_number: key,
                subtype: subtype.to_string(),
                grading_mode: grading_mode_label(info.grading_mode),
                correct,
                score: ai_score,
                pending_review,
                points_earned: earned,
                points_max: max,
            });
        }
    }

    let score = if points_possible > 0.0 { Some((points_earned / points_possible * 1000.0).round() / 10.0) } else { None };
    let status = if has_pending { "submitted" } else { "evaluated" };

    sqlx::query!(
        r#"update attempts set answers = $2, score = $3, status = $4, submitted_at = now(), question_snapshot = $5 where id = $1"#,
        attempt_id,
        serde_json::Value::Object(answers_snapshot),
        score,
        status,
        raw_config,
    )
    .execute(pool)
    .await?;

    // What the access gates read. A score still waiting on a teacher is
    // recorded once graded (grade_manual_group).
    crate::services::item_progress::record_completion(pool, ctx.user_id, item_id, if has_pending { None } else { score }).await?;
    if let Some(session_id) = proctor_session {
        crate::services::item_proctor::close_session(pool, session_id, attempt_id).await?;
    }

    Ok(QuizSubmitResponse { attempt_id, status: status.to_string(), score, results })
}

/// Run the subtype's existing LLM evaluator over one submitted answer.
/// Returns the 0-100 overall, or None when there was nothing to grade.
#[allow(clippy::too_many_arguments)]
async fn evaluate_with_rubric(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    ctx: &AuthContext,
    attempt_id: Uuid,
    subtype: &str,
    submitted: &serde_json::Value,
) -> Result<Option<f64>, AppError> {
    match subtype {
        "essay" | "sentence_transform" => {
            let text = answer_text(submitted);
            if text.trim().is_empty() {
                return Ok(None);
            }
            Ok(ai_writing_evaluation::run_writing_evaluation(
                pool,
                ai,
                &config.ai_writing_evaluation_model,
                ctx.user_id,
                attempt_id,
                text,
                // The evaluator takes a rubric ROW id; a question's
                // free-text `rubric` is authoring guidance for the
                // generator, not a stored rubric, so the default applies.
                None,
            )
            .await
            .map(|r| r.evaluation.scores.overall))
        }
        "voice_record" | "speaking_challenge" | "listen_repeat" | "intonation" => {
            let Some(asset_id) = answer_asset_id(submitted) else { return Ok(None) };
            let Some((bytes, content_type)) = read_audio_bytes(pool, storage, ctx, asset_id).await? else {
                return Ok(None);
            };
            let result = ai_speaking_evaluation::run_speaking_evaluation(pool, config, ai, ctx.user_id, attempt_id, &bytes, &content_type, None).await;
            Ok(result.evaluation.map(|e| e.scores.overall))
        }
        _ => Ok(None),
    }
}

// --- Manual (teacher) grading queue ---
// Reuses the existing evaluations/feedback tables (evaluator_type
// already allows 'human', just never written before this) rather than
// a new table — a teacher's score is structurally the same "evidence +
// scores + feedback" shape as an AI evaluation, just typed 'human'.

#[derive(Debug, serde::Serialize)]
pub struct PendingReviewRow {
    pub attempt_id: Uuid,
    pub student_name: String,
    pub group_id: String,
    /// Identifies WHICH question in the group still needs a verdict.
    pub question_number: String,
    pub subtype: String,
    pub submitted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub answer: serde_json::Value,
}

// GET /module-items/{id}/pending-reviews — every `submitted`-status
// attempt on this quiz item that still has an un-evaluated Manual
// group, one row per (attempt, group) pair still awaiting a teacher.
pub async fn list_pending_reviews(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<Vec<PendingReviewRow>, AppError> {
    require_permission(ctx, Resource::Attempt, Action::Submit)?;

    struct Row {
        id: Uuid,
        user_id: Uuid,
        answers: serde_json::Value,
        submitted_at: Option<chrono::DateTime<chrono::Utc>>,
        question_snapshot: Option<serde_json::Value>,
    }
    let attempts = sqlx::query_as!(
        Row,
        r#"select a.id, a.user_id, a.answers, a.submitted_at, a.question_snapshot
           from attempts a where a.item_id = $1 and a.status = 'submitted'"#,
        item_id,
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::new();
    for attempt in attempts {
        let Some(snapshot) = &attempt.question_snapshot else { continue };
        let Some(groups) = snapshot.get("question_groups").and_then(|v| v.as_array()) else { continue };
        let evaluated_group_ids: std::collections::HashSet<String> = sqlx::query_scalar!(
            r#"select evidence->>'group_id' as "group_id!" from evaluations where attempt_id = $1 and evidence ? 'group_id'"#,
            attempt.id,
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();

        let student_name = sqlx::query_scalar!(r#"select name from users where id = $1"#, attempt.user_id).fetch_optional(pool).await?.unwrap_or_default();

        for group in groups {
            let Some(group_id) = group.get("group_id").and_then(|v| v.as_str()) else { continue };
            let Some(subtype) = group.get("type").and_then(|v| v.as_str()) else { continue };
            let Some(info) = quiz_subtype::find(subtype) else { continue };
            if info.grading_mode != quiz_subtype::GradingMode::Manual {
                continue;
            }
            // One row per QUESTION: a file_upload group with three
            // uploads needs three separate teacher verdicts, not one.
            let questions = group.get("questions").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            for question in questions {
                let Some(number) = question.get("number") else { continue };
                let key = value_to_key(number);
                let review_key = format!("{group_id}#{key}");
                if evaluated_group_ids.contains(&review_key) {
                    continue;
                }
                let answer = attempt.answers.get(&key).cloned().unwrap_or(serde_json::Value::Null);
                out.push(PendingReviewRow {
                    attempt_id: attempt.id,
                    student_name: student_name.clone(),
                    group_id: group_id.to_string(),
                    question_number: key,
                    subtype: subtype.to_string(),
                    submitted_at: attempt.submitted_at,
                    answer,
                });
            }
        }
    }
    Ok(out)
}

// POST /attempts/{id}/grade — a teacher scores one Manual-mode group
// (0-100 + optional feedback comment). Once every Manual group on an
// attempt has a 'human' evaluation, the attempt's overall status/score
// flips to 'evaluated', folding the manual score(s) into the same
// aggregate the auto/ai_rubric groups already contributed to.
pub async fn grade_manual_group(pool: &PgPool, ctx: &AuthContext, attempt_id: Uuid, group_id: &str, question_number: &str, score: f64, feedback: Option<String>) -> Result<(), AppError> {
    require_permission(ctx, Resource::Attempt, Action::Submit)?;
    if !(0.0..=100.0).contains(&score) {
        return Err(AppError::UnprocessableEntity("invalid_score", "score must be between 0 and 100".to_string()));
    }

    let attempt = sqlx::query!(r#"select score, question_snapshot, user_id, item_id from attempts where id = $1"#, attempt_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("attempt_not_found"))?;

    let rubric = ensure_manual_review_rubric(pool).await?;
    let scores_json = serde_json::json!({"quality": score});
    // Keyed per QUESTION — `list_pending_reviews` builds the same
    // "{group}#{number}" key to decide what still needs a verdict, so a
    // group-only key would mark every sibling question graded at once.
    let review_key = format!("{group_id}#{question_number}");
    let evidence = serde_json::json!({"group_id": review_key, "graded_by": ctx.user_id});
    let inserted = evaluation::insert_evaluation(pool, attempt_id, "human", rubric.id, scores_json, evidence, None).await?;
    if let Some(comment) = feedback.filter(|f| !f.trim().is_empty()) {
        evaluation::insert_many_feedback(pool, inserted.id, &[evaluation::FeedbackItemInput { content: comment, position: None }]).await?;
    }

    // Re-check whether every Manual group on this attempt now has a
    // 'human' evaluation — if so, fold this score into the aggregate
    // and flip the attempt to 'evaluated'.
    let Some(snapshot) = attempt.question_snapshot else { return Ok(()) };
    let Some(groups) = snapshot.get("question_groups").and_then(|v| v.as_array()) else { return Ok(()) };
    // Counted per QUESTION, matching how submit_quiz_attempt built
    // points_possible — counting groups here while submission counted
    // questions would weight a 10-question passage the same as a
    // 1-question essay.
    let mut manual_question_count = 0usize;
    // Only Auto/AiRubric questions fed attempts.score's denominator at
    // submission time (SelfCheck never counts), so the reaggregation
    // reuses that same denominator rather than "everything non-manual".
    let mut scored_question_count = 0usize;
    for g in groups {
        let Some(subtype) = g.get("type").and_then(|v| v.as_str()) else { continue };
        let Some(info) = quiz_subtype::find(subtype) else { continue };
        let question_count = g.get("questions").and_then(|v| v.as_array()).map_or(0, Vec::len);
        match info.grading_mode {
            quiz_subtype::GradingMode::Manual => manual_question_count += question_count,
            quiz_subtype::GradingMode::Auto | quiz_subtype::GradingMode::AiRubric => scored_question_count += question_count,
            quiz_subtype::GradingMode::SelfCheck => {}
        }
    }

    let graded_scores: Vec<f64> = sqlx::query!(
        r#"select scores->>'quality' as "quality!" from evaluations where attempt_id = $1 and evaluator_type = 'human' and evidence ? 'group_id'"#,
        attempt_id,
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .filter_map(|r| r.quality.parse::<f64>().ok())
    .collect();

    if graded_scores.len() < manual_question_count {
        return Ok(());
    }

    // Fold every graded Manual score (0-100 each) in with whatever the
    // existing attempts.score already reflects from Auto/AiRubric
    // questions at submission time — a question-count-weighted average
    // across every SCORED question (self-check ones carry no weight,
    // same as they carried none in submit_quiz_attempt).
    let manual_sum: f64 = graded_scores.iter().sum();
    let existing_score = attempt.score.unwrap_or(0.0);
    let combined = if scored_question_count > 0 {
        (existing_score * scored_question_count as f64 + manual_sum) / (scored_question_count as f64 + graded_scores.len() as f64)
    } else {
        manual_sum / graded_scores.len() as f64
    };
    let combined = (combined * 10.0).round() / 10.0;

    sqlx::query!(r#"update attempts set status = 'evaluated', score = $2 where id = $1"#, attempt_id, combined).execute(pool).await?;
    // Now that the score is final, a "lulus dengan skor X%" gate can use it.
    if let Some(item_id) = attempt.item_id {
        crate::services::item_progress::record_completion(pool, attempt.user_id, item_id, Some(combined)).await?;
    }
    Ok(())
}
