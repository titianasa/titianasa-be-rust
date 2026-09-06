use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::assessment::{self, AttemptQuestion};

// Port of exam_service.ts + exam_repository.ts. P7-001 — a "sesi ujian"
// layered on top of the existing attempts flow: timer/deadline
// bookkeeping, deliberately separate from Proctoring (R11) by schema
// design. Grading itself is still 100% assessment_service::submit_attempt
// (not yet ported — see R4's own documented deferral; this module never
// re-implements or duplicates grading).

#[derive(Debug, Clone)]
pub struct ExamSessionRow {
    pub id: Uuid,
    pub assessment_id: Uuid,
    pub user_id: Uuid,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ExamSessionRow>, AppError> {
    let row = sqlx::query_as!(ExamSessionRow, r#"select id, assessment_id, user_id, status, started_at, submitted_at from exam_sessions where id = $1"#, id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

// duration_minutes lives inside assessments.config (jsonb, free-form)
// rather than a dedicated column. Absent/invalid means "untimed" —
// exam_sessions still records start/submit, just never times out.
fn read_duration_minutes(config: &serde_json::Value) -> Option<i64> {
    let value = config.get("duration_minutes")?.as_f64()?;
    if value > 0.0 {
        Some(value as i64)
    } else {
        None
    }
}

fn compute_deadline(started_at: Option<DateTime<Utc>>, duration_minutes: Option<i64>) -> Option<DateTime<Utc>> {
    let started_at = started_at?;
    let duration_minutes = duration_minutes?;
    Some(started_at + chrono::Duration::minutes(duration_minutes))
}

#[derive(Debug, serde::Serialize)]
pub struct StartExamSessionResponse {
    pub exam_session_id: Uuid,
    pub attempt_id: Uuid,
    pub status: String,
    pub deadline: Option<DateTime<Utc>>,
    pub questions: Vec<AttemptQuestion>,
}

// POST /assessments/{id}/exam-sessions — reuses assessment::create_attempt
// outright (same permission check, same in-progress dedup, same question
// list) rather than duplicating any of it: an exam session is that same
// attempt plus timer bookkeeping, not a parallel creation path.
pub async fn start_exam_session(pool: &PgPool, ctx: &AuthContext, assessment_id: Uuid) -> Result<StartExamSessionResponse, AppError> {
    let config = sqlx::query_scalar!(r#"select config from assessments where id = $1"#, assessment_id).fetch_optional(pool).await?;
    let Some(config) = config else { return Err(AppError::NotFound("assessment_not_found")) };

    let attempt = assessment::create_attempt(pool, ctx, assessment_id).await?;

    let session = sqlx::query!(
        r#"insert into exam_sessions (assessment_id, user_id, status, started_at) values ($1, $2, 'in_progress', now())
           returning id, status, started_at"#,
        assessment_id,
        ctx.user_id,
    )
    .fetch_one(pool)
    .await?;

    let duration_minutes = read_duration_minutes(&config);
    let deadline = compute_deadline(session.started_at, duration_minutes);

    Ok(StartExamSessionResponse { exam_session_id: session.id, attempt_id: attempt.attempt_id, status: session.status, deadline, questions: attempt.questions })
}

#[derive(Debug, serde::Serialize)]
pub struct ExamSessionResponse {
    pub id: Uuid,
    pub assessment_id: Uuid,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub deadline: Option<DateTime<Utc>>,
}

async fn to_response(pool: &PgPool, session: ExamSessionRow) -> Result<ExamSessionResponse, AppError> {
    let config = sqlx::query_scalar!(r#"select config from assessments where id = $1"#, session.assessment_id).fetch_optional(pool).await?;
    let duration_minutes = config.as_ref().and_then(read_duration_minutes);
    let deadline = compute_deadline(session.started_at, duration_minutes);
    Ok(ExamSessionResponse { id: session.id, assessment_id: session.assessment_id, status: session.status, started_at: session.started_at, submitted_at: session.submitted_at, deadline })
}

// GET /exam-sessions/{id} — ownership check only ("milik sendiri"), no
// separate permission tier for reading a session you started yourself.
pub async fn get_exam_session(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<ExamSessionResponse, AppError> {
    let session = find_by_id(pool, id).await?.ok_or(AppError::NotFound("exam_session_not_found"))?;
    if session.user_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }
    to_response(pool, session).await
}

async fn find_active_for_assessment(pool: &PgPool, assessment_id: Uuid, user_id: Uuid) -> Result<Option<ExamSessionRow>, AppError> {
    let row = sqlx::query_as!(
        ExamSessionRow,
        r#"select id, assessment_id, user_id, status, started_at, submitted_at from exam_sessions
           where assessment_id = $1 and user_id = $2 and status = 'in_progress'
           order by started_at desc limit 1"#,
        assessment_id,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// Called (unconditionally) from the assessment-path submit handler
// right after a successful submit. A no-op for the common case: most
// submits aren't part of a timed session. Deliberately never touches
// attempts.score/grading — a late submission is still accepted and
// graded exactly as submitted (auto-submit-on-timeout, not a rejection).
pub async fn record_submission(pool: &PgPool, assessment_id: Uuid, user_id: Uuid, submitted_at: DateTime<Utc>) -> Result<(), AppError> {
    let Some(session) = find_active_for_assessment(pool, assessment_id, user_id).await? else { return Ok(()) };
    let config = sqlx::query_scalar!(r#"select config from assessments where id = $1"#, assessment_id).fetch_optional(pool).await?;
    let duration_minutes = config.as_ref().and_then(read_duration_minutes);
    let deadline = compute_deadline(session.started_at, duration_minutes);
    let timed_out = deadline.map(|d| submitted_at > d).unwrap_or(false);
    let status = if timed_out { "timed_out" } else { "submitted" };
    sqlx::query!(r#"update exam_sessions set status = $2, submitted_at = $3 where id = $1"#, session.id, status, submitted_at).execute(pool).await?;
    Ok(())
}
