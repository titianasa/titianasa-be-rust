use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::responses::assessment::AssessmentSummary;
use crate::services::permissions::{is_allowed, require_permission, Action, Resource};
use crate::services::quiz_config_schema;

pub struct NewAssessment {
    pub r#type: String,
    pub title: String,
    pub config: serde_json::Value,
    pub question_ids: Vec<Uuid>,
}

// POST /assessments — P2-016. No permission row exists for this in
// ADR-0006 (assessment authoring was never designed as its own
// resource) — reuses question_bank:create's tier since bundling
// already-authored questions into an assessment is the same kind of
// authoring work.
pub async fn create_assessment(pool: &PgPool, ctx: &AuthContext, new_assessment: NewAssessment) -> Result<AssessmentSummary, AppError> {
    require_permission(ctx, Resource::QuestionBank, Action::Create)?;

    if new_assessment.question_ids.is_empty() {
        return Err(AppError::UnprocessableEntity("invalid_assessment", "question_ids must have at least 1 entry".to_string()));
    }

    let row = sqlx::query!(
        r#"insert into assessments (type, title, config) values ($1, $2, $3) returning id, type, title, config"#,
        new_assessment.r#type,
        new_assessment.title,
        new_assessment.config,
    )
    .fetch_one(pool)
    .await?;

    for (i, question_id) in new_assessment.question_ids.iter().enumerate() {
        sqlx::query!(
            r#"insert into assessment_questions (assessment_id, question_id, order_index) values ($1, $2, $3)"#,
            row.id,
            question_id,
            i as i32,
        )
        .execute(pool)
        .await?;
    }

    Ok(AssessmentSummary {
        id: row.id,
        r#type: row.r#type,
        title: row.title,
        config: row.config,
        question_count: new_assessment.question_ids.len() as i64,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct AttemptQuestion {
    pub id: Uuid,
    pub r#type: String,
    pub data: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
pub struct CreateAttemptResponse {
    pub attempt_id: Uuid,
    pub status: String,
    pub questions: Vec<AttemptQuestion>,
}

// POST /assessments/{id}/attempts
pub async fn create_attempt(pool: &PgPool, ctx: &AuthContext, assessment_id: Uuid) -> Result<CreateAttemptResponse, AppError> {
    require_permission(ctx, Resource::Attempt, Action::Create)?;

    let exists = sqlx::query_scalar!(r#"select id from assessments where id = $1"#, assessment_id).fetch_optional(pool).await?;
    if exists.is_none() {
        return Err(AppError::NotFound("assessment_not_found"));
    }

    let existing = sqlx::query_scalar!(r#"select id from attempts where user_id = $1 and assessment_id = $2 and status = 'in_progress'"#, ctx.user_id, assessment_id)
        .fetch_optional(pool)
        .await?;
    if let Some(existing_id) = existing {
        return Err(AppError::AttemptAlreadyInProgress(existing_id));
    }

    let attempt = sqlx::query!(r#"insert into attempts (user_id, assessment_id) values ($1, $2) returning id, status"#, ctx.user_id, assessment_id).fetch_one(pool).await?;

    let questions = sqlx::query_as!(
        AttemptQuestion,
        r#"select q.id, q.type, q.data from questions q
           inner join assessment_questions aq on aq.question_id = q.id
           where aq.assessment_id = $1
           order by aq.order_index asc"#,
        assessment_id,
    )
    .fetch_all(pool)
    .await?;

    Ok(CreateAttemptResponse { attempt_id: attempt.id, status: attempt.status, questions })
}

// GET /assessments/{id}
pub async fn get_assessment(pool: &PgPool, assessment_id: Uuid) -> Result<AssessmentSummary, AppError> {
    let row = sqlx::query!(r#"select id, type, title, config from assessments where id = $1"#, assessment_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("assessment_not_found"))?;

    let question_count = sqlx::query_scalar!(
        r#"select count(*) as "count!" from assessment_questions where assessment_id = $1"#,
        assessment_id,
    )
    .fetch_one(pool)
    .await?;

    Ok(AssessmentSummary { id: row.id, r#type: row.r#type, title: row.title, config: row.config, question_count })
}

pub struct AttemptRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub assessment_id: Option<Uuid>,
    pub item_id: Option<Uuid>,
    pub status: String,
    /// Sat from /belajar/{id}/preview — see migrations/0056.
    pub is_preview: bool,
}

pub async fn find_attempt(pool: &PgPool, id: Uuid) -> Result<Option<AttemptRow>, AppError> {
    let row = sqlx::query_as!(AttemptRow, r#"select id, user_id, assessment_id, item_id, status, is_preview from attempts where id = $1"#, id).fetch_optional(pool).await?;
    Ok(row)
}

// Small standalone lookup so the submit handler's post-grading XP hook
// doesn't need to re-run get_assessment's full question_count query just
// to learn the assessment's type.
pub async fn find_assessment_type(pool: &PgPool, assessment_id: Uuid) -> Result<Option<String>, AppError> {
    let r#type = sqlx::query_scalar!(r#"select type from assessments where id = $1"#, assessment_id).fetch_optional(pool).await?;
    Ok(r#type)
}

// Shared by submit_attempt, submit_writing_attempt, submit_speaking_attempt.
pub async fn load_submittable_attempt(pool: &PgPool, ctx: &AuthContext, attempt_id: Uuid) -> Result<AttemptRow, AppError> {
    let attempt = find_attempt(pool, attempt_id).await?.ok_or(AppError::NotFound("attempt_not_found"))?;
    if attempt.user_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }
    if attempt.status != "in_progress" {
        return Err(AppError::Conflict("attempt_already_submitted"));
    }
    Ok(attempt)
}

#[derive(Debug, serde::Serialize)]
pub struct CreateLessonAttemptResponse {
    pub attempt_id: Uuid,
    pub status: String,
    /// The learner-safe quiz the client renders for THIS attempt: drawn,
    /// shuffled, re-lettered, no answer keys (`quiz_paper`). None only
    /// for an item whose config can't be parsed.
    pub paper: Option<serde_json::Value>,
    /// This sitting is an author previewing a draft: real paper, real
    /// grading, no XP. The client labels it so a preview is never
    /// mistaken for a learner's own attempt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview: bool,
}

/// The item a quiz's questions actually live in — itself, or the bank a
/// `question_pool` points at. The bank must be a quiz in the same module,
/// which `module_item::update_quiz_config` already enforces on save; it is
/// re-checked here because a bank can be moved or retyped afterwards.
async fn resolve_question_source(pool: &PgPool, item_id: Uuid, policy: &crate::services::quiz_config::QuizConfig, own_raw: &serde_json::Value) -> Result<(Uuid, serde_json::Value), AppError> {
    let Some(source_id) = policy.question_pool.as_ref().and_then(|p| p.source_item_id).filter(|id| *id != item_id) else {
        return Ok((item_id, own_raw.clone()));
    };
    let row = sqlx::query!(
        r#"select s.quiz_config from module_items s join module_items i on i.module_id = s.module_id
           where s.id = $1 and i.id = $2 and s.content_type = 'quiz'"#,
        source_id,
        item_id,
    )
    .fetch_optional(pool)
    .await?;
    match row.and_then(|r| r.quiz_config) {
        Some(raw) => Ok((source_id, raw)),
        None => Err(AppError::UnprocessableEntity("question_pool_source_missing", "bank soal untuk kuis ini tidak ditemukan".to_string())),
    }
}

/// Builds and stores the paper for `attempt_id`. The learner's previous
/// paper from the same source is read first so a retake leans towards
/// questions they have not just seen.
async fn assign_paper(pool: &PgPool, user_id: Uuid, item_id: Uuid, attempt_id: Uuid, own_raw: &serde_json::Value, preview: bool) -> Result<Option<serde_json::Value>, AppError> {
    let Ok(policy) = quiz_config_schema::parse(own_raw) else { return Ok(None) };
    let (source_item_id, source_raw) = resolve_question_source(pool, item_id, &policy, own_raw).await?;
    let source = quiz_config_schema::parse(&source_raw)?;

    let previous = sqlx::query_scalar!(
        r#"select paper from attempts
           where user_id = $1 and id <> $2 and paper->>'source_item_id' = $3 and is_preview = $4
           order by started_at desc limit 1"#,
        user_id,
        attempt_id,
        source_item_id.to_string(),
        preview,
    )
    .fetch_optional(pool)
    .await?
    .flatten();
    let recently_seen: std::collections::HashSet<Uuid> = previous
        .and_then(|v| serde_json::from_value::<crate::services::quiz_paper::Paper>(v).ok())
        .map(|p| p.questions.into_iter().map(|q| q.uid).collect())
        .unwrap_or_default();

    let paper = crate::services::quiz_paper::build(crate::services::quiz_paper::PaperInput {
        source_item_id,
        source: &source,
        policy: &policy,
        recently_seen: &recently_seen,
        seed: crate::services::quiz_paper::seed_from(attempt_id),
    });
    let config = paper.config.clone();
    let stored = serde_json::to_value(&paper).map_err(|e| AppError::Internal(e.into()))?;
    sqlx::query!(r#"update attempts set paper = $2 where id = $1"#, attempt_id, stored).execute(pool).await?;
    Ok(Some(config))
}

/// Deletes the preview sittings on `item_ids` and what hangs off them
/// (AI evaluations and their feedback). Only previews: learner attempts
/// are history and are what the delete paths refuse over.
pub async fn purge_preview_attempts(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, item_ids: &[Uuid]) -> Result<(), AppError> {
    if item_ids.is_empty() {
        return Ok(());
    }
    let preview_ids: Vec<Uuid> = sqlx::query_scalar!(r#"select id from attempts where is_preview and item_id = any($1)"#, item_ids).fetch_all(&mut **tx).await?;
    if preview_ids.is_empty() {
        return Ok(());
    }
    sqlx::query!(r#"delete from feedback where evaluation_id in (select id from evaluations where attempt_id = any($1))"#, &preview_ids).execute(&mut **tx).await?;
    sqlx::query!(r#"delete from evaluations where attempt_id = any($1)"#, &preview_ids).execute(&mut **tx).await?;
    sqlx::query!(r#"update canvas_sessions set submitted_attempt_id = null where submitted_attempt_id = any($1)"#, &preview_ids).execute(&mut **tx).await?;
    sqlx::query!(r#"delete from attempts where id = any($1)"#, &preview_ids).execute(&mut **tx).await?;
    Ok(())
}

/// May this caller open `item_id` in preview mode (/belajar/{id}/preview):
/// an authoring role that can see unpublished content (author,
/// reviewer, admin) or a collaborator shared onto the item. Status does
/// not matter — a published quiz is previewed the same way as a draft.
pub async fn can_preview(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<bool, AppError> {
    if is_allowed(ctx.role.as_deref(), Resource::ModuleItem, Action::ViewUnpublished) {
        return Ok(true);
    }
    crate::services::resource_share::has_module_item_grant(pool, ctx.user_id, ctx.role.as_deref(), item_id, "viewer").await
}

// POST /lessons/{id}/attempts — called when the learner presses "Mulai".
// An attempt already in progress is handed back with the paper it was
// given, so a refresh (or a second tab) resumes the same paper instead of
// re-rolling a new draw or getting stuck behind a 409.
pub async fn create_lesson_attempt(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, preview: bool) -> Result<CreateLessonAttemptResponse, AppError> {
    let item = sqlx::query!(r#"select content_type, status, quiz_config from module_items where id = $1"#, item_id).fetch_optional(pool).await?;
    let Some(item) = item else { return Err(AppError::NotFound("module_item_not_found")) };
    // Phase 37 — an "attempt" only ever applies to a quiz item now
    // (an article has no submission concept). Canvas sessions
    // (canvas.rs::submit_session) reuse this same call for their
    // essay-subtype quiz items.
    if item.content_type.as_deref() != Some("quiz") {
        return Err(AppError::UnprocessableEntity("invalid_lesson_type", r#"module_item.content_type must be "quiz""#.to_string()));
    }
    // Two doors, never inferred from each other. The learner door needs
    // a learner's permission and a published quiz. The preview door
    // (/belajar/{id}/preview) needs the right to see unpublished content,
    // works on any status, and its sitting is marked so nothing it does
    // counts as learner history.
    if preview {
        if !can_preview(pool, ctx, item_id).await? {
            return Err(AppError::ForbiddenWithCode("preview_not_allowed"));
        }
    } else {
        require_permission(ctx, Resource::Attempt, Action::Create)?;
        if item.status != "published" {
            return Err(AppError::ForbiddenWithCode("lesson_not_published"));
        }
    }
    let own_raw = item.quiz_config.clone().unwrap_or(serde_json::Value::Null);

    let existing = sqlx::query!(r#"select id, status, paper from attempts where user_id = $1 and item_id = $2 and status = 'in_progress' and is_preview = $3"#, ctx.user_id, item_id, preview)
        .fetch_optional(pool)
        .await?;
    if let Some(existing) = existing {
        let paper = match existing.paper.and_then(|v| serde_json::from_value::<crate::services::quiz_paper::Paper>(v).ok()) {
            Some(paper) => Some(paper.config),
            // Started before papers existed — give it one now.
            None => assign_paper(pool, ctx.user_id, item_id, existing.id, &own_raw, preview).await?,
        };
        return Ok(CreateLessonAttemptResponse { attempt_id: existing.id, status: existing.status, paper, preview });
    }

    // Phase 38 — "Maks. Percobaan": `quiz_config.max_attempts` caps how
    // many times a learner may ever START this quiz. Absent/None keeps
    // today's behavior (unlimited). A malformed quiz_config is not this
    // check's job to reject — that's PATCH's job — so it's treated the
    // same as "no cap" rather than blocking every attempt.
    if let Some(max_attempts) = item.quiz_config.as_ref().filter(|_| !preview).and_then(|raw| quiz_config_schema::parse(raw).ok()).and_then(|quiz| quiz.max_attempts) {
        let attempt_count = sqlx::query_scalar!(r#"select count(*) as "count!" from attempts where user_id = $1 and item_id = $2 and not is_preview"#, ctx.user_id, item_id).fetch_one(pool).await?;
        if attempt_count >= max_attempts {
            return Err(AppError::UnprocessableEntity(
                "max_attempts_reached",
                format!("kamu sudah mencapai batas maksimal {max_attempts} percobaan untuk kuis ini"),
            ));
        }
    }

    let attempt = sqlx::query!(r#"insert into attempts (user_id, item_id, is_preview) values ($1, $2, $3) returning id, status"#, ctx.user_id, item_id, preview).fetch_one(pool).await?;
    let paper = assign_paper(pool, ctx.user_id, item_id, attempt.id, &own_raw, preview).await?;
    Ok(CreateLessonAttemptResponse { attempt_id: attempt.id, status: attempt.status, paper, preview })
}

pub async fn submit_lesson_attempt(pool: &PgPool, attempt_id: Uuid, answer_text: &str) -> Result<(), AppError> {
    let answers = serde_json::json!({"text": answer_text});
    sqlx::query!(r#"update attempts set answers = $2, status = 'submitted', submitted_at = now() where id = $1"#, attempt_id, answers).execute(pool).await?;
    Ok(())
}

pub async fn mark_attempt_evaluated(pool: &PgPool, attempt_id: Uuid, score: f64) -> Result<(), AppError> {
    sqlx::query!(r#"update attempts set status = 'evaluated', score = $2 where id = $1"#, attempt_id, score).execute(pool).await?;
    Ok(())
}

// ASSESSMENT_XP (xp_service.ts) — per-assessment-type XP, distinct from
// xp::skill_xp (per-skill-category). Lives here, not xp.rs, since it's
// only ever consulted from this submit path.
pub fn assessment_xp(assessment_type: &str) -> i32 {
    match assessment_type {
        "mock_exam" => 30,
        "level_assessment" => 30,
        _ => 20, // unit_test and any other/unknown type
    }
}

#[derive(Debug, serde::Serialize)]
pub struct SubmitAttemptResponse {
    pub attempt_id: Uuid,
    pub status: String,
    pub score: f64,
    pub learning_events_created: i64,
}

// POST /attempts/{id}/submit — the plain-assessment grading path
// (unit_test/mock_exam). `assessments.type == 'level_assessment'`
// (ADR-0011 composite Knowledge/Communication scoring) is a genuinely
// separate grading algorithm — deliberately DEFERRED, not
// half-implemented: this function returns a distinct, honest error for
// that type rather than silently mis-grading it with the plain-MCQ
// path.
pub async fn submit_attempt(pool: &PgPool, config: &Config, ctx: &AuthContext, attempt_id: Uuid, answers: &std::collections::HashMap<Uuid, serde_json::Value>) -> Result<SubmitAttemptResponse, AppError> {
    require_permission(ctx, Resource::Attempt, Action::Submit)?;
    let attempt = load_submittable_attempt(pool, ctx, attempt_id).await?;
    let Some(assessment_id) = attempt.assessment_id else { return Err(AppError::Internal(anyhow::anyhow!("attempt has no assessment_id"))) };

    let assessment_type = sqlx::query_scalar!(r#"select type from assessments where id = $1"#, assessment_id).fetch_one(pool).await?;
    if assessment_type == "level_assessment" {
        return Err(AppError::UnprocessableEntity(
            "level_assessment_scoring_not_implemented",
            "Level Assessment composite (Knowledge/Communication) scoring is not yet ported — see phase-30-rust-migration.md".to_string(),
        ));
    }

    struct QuestionRow {
        id: Uuid,
        r#type: String,
        difficulty: f64,
        correct_answer: serde_json::Value,
        data: serde_json::Value,
        explanation: Option<serde_json::Value>,
        version: i32,
    }
    let questions = sqlx::query_as!(
        QuestionRow,
        r#"select q.id, q.type, q.difficulty, q.correct_answer, q.data, q.explanation, q.version
           from questions q inner join assessment_questions aq on aq.question_id = q.id
           where aq.assessment_id = $1 order by aq.order_index asc"#,
        assessment_id,
    )
    .fetch_all(pool)
    .await?;

    let missing: Vec<Uuid> = questions.iter().filter(|q| !answers.contains_key(&q.id)).map(|q| q.id).collect();
    if !missing.is_empty() {
        return Err(AppError::MissingRequiredAnswers(missing));
    }

    let question_ids: Vec<Uuid> = questions.iter().map(|q| q.id).collect();
    let concept_rows = sqlx::query!(r#"select question_id, concept_id from question_concepts where question_id = any($1)"#, &question_ids).fetch_all(pool).await?;
    let mut concept_ids_by_question: std::collections::HashMap<Uuid, Vec<Uuid>> = std::collections::HashMap::new();
    for row in concept_rows {
        concept_ids_by_question.entry(row.question_id).or_default().push(row.concept_id);
    }

    let mut points_possible: f64 = 0.0;
    let mut points_earned: f64 = 0.0;
    let mut touched_concepts: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    let mut concept_correctness: std::collections::HashMap<Uuid, Vec<f64>> = std::collections::HashMap::new();
    // P39-003 — event_type is always "question_answered" here today,
    // but kept alongside entity_id/payload (rather than assumed) so a
    // second event type could join this same loop later without
    // reshaping it.
    let mut events: Vec<(&'static str, Uuid, serde_json::Value)> = Vec::new();
    let mut question_snapshot = serde_json::Map::new();

    for q in &questions {
        let submitted = answers.get(&q.id).cloned().unwrap_or(serde_json::Value::Null);
        let correct: Option<bool> = if crate::services::grading::is_auto_gradable(&q.r#type) {
            let is_correct_answer = crate::services::grading::is_correct(&q.r#type, &q.correct_answer, &submitted, Some(&q.data));
            points_possible += 1.0;
            if is_correct_answer {
                points_earned += 1.0;
            }
            Some(is_correct_answer)
        } else {
            None
        };

        let concept_ids = concept_ids_by_question.get(&q.id).cloned().unwrap_or_default();
        for concept_id in &concept_ids {
            touched_concepts.insert(*concept_id);
            if let Some(c) = correct {
                concept_correctness.entry(*concept_id).or_default().push(if c { 1.0 } else { 0.0 });
            }
        }

        events.push(("question_answered", q.id, serde_json::json!({"correct": correct, "difficulty": q.difficulty, "question_id": q.id, "concept_ids": concept_ids})));
        question_snapshot.insert(q.id.to_string(), serde_json::json!({"data": q.data, "correct_answer": q.correct_answer, "explanation": q.explanation, "version": q.version}));
    }

    let score = if points_possible > 0.0 { (points_earned / points_possible * 1000.0).round() / 10.0 } else { 0.0 };
    let answers_json: serde_json::Map<String, serde_json::Value> = answers.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();

    sqlx::query!(
        r#"update attempts set answers = $2, score = $3, status = 'submitted', submitted_at = now(), question_snapshot = $4 where id = $1"#,
        attempt_id,
        serde_json::Value::Object(answers_json),
        score,
        serde_json::Value::Object(question_snapshot),
    )
    .execute(pool)
    .await?;

    // P39-003 — routed through the registry (learning_event::record)
    // instead of a raw insert, so an unrecognised event type or a
    // payload missing what mastery.rs/frss.rs actually read would be
    // caught here rather than silently stored. The payload shape below
    // is UNCHANGED from before this migration — see learning_event.rs's
    // own header for exactly which keys those readers depend on.
    let mut learning_events_created = 0i64;
    for (event_type, question_id, payload) in &events {
        crate::services::learning_event::record(
            pool,
            ctx.user_id,
            crate::services::learning_event::EventChannel::Server,
            crate::services::learning_event::NewLearningEvent::server(event_type, "question", *question_id, payload.clone(), "practice"),
        )
        .await?;
        learning_events_created += 1;
    }

    for concept_id in &touched_concepts {
        crate::services::mastery::recompute_for_concept(pool, config, ctx.user_id, *concept_id).await?;
    }
    for (concept_id, values) in &concept_correctness {
        let avg = values.iter().sum::<f64>() / values.len() as f64;
        crate::services::frss::record_review(pool, config, ctx.user_id, *concept_id, avg).await?;
    }

    Ok(SubmitAttemptResponse { attempt_id, status: "submitted".to_string(), score, learning_events_created })
}
