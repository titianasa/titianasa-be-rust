use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::cohort::can_manage_cohorts;
use crate::services::meeting_provider::MeetingProvider;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClassSessionResponse {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub session_date: String,
    pub scheduled_start: DateTime<Utc>,
    pub scheduled_end: DateTime<Utc>,
    pub meeting_provider: String,
    pub external_meeting_id: Option<String>,
    pub join_url: Option<String>,
    pub status: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ClassSessionListResponse {
    pub items: Vec<ClassSessionResponse>,
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ClassSessionResponse>, AppError> {
    let row = sqlx::query_as!(
        ClassSessionResponse,
        r#"select id, cohort_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url, status
           from class_sessions where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn list_by_cohort(pool: &PgPool, cohort_id: Uuid) -> Result<Vec<ClassSessionResponse>, AppError> {
    let rows = sqlx::query_as!(
        ClassSessionResponse,
        r#"select id, cohort_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url, status
           from class_sessions where cohort_id = $1"#,
        cohort_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> Result<ClassSessionResponse, AppError> {
    let row = sqlx::query_as!(
        ClassSessionResponse,
        r#"update class_sessions set status = $2 where id = $1
           returning id, cohort_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url, status"#,
        id,
        status,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub(crate) async fn assert_can_manage_cohort(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid) -> Result<(), AppError> {
    let (_cohort, product) = crate::services::cohort::find_product_for_cohort_id(pool, cohort_id).await?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

// Manager OR any enrolled student (any status) — a schedule + join_url
// carries no other student's private data, unlike attendance/gradebook
// which stay manager-or-own. This is the one genuinely NEW row-context
// check R8 introduces (everything else reuses can_manage_cohorts as-is).
pub(crate) async fn assert_can_manage_cohort_or_enrolled(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid) -> Result<(), AppError> {
    let (_cohort, product) = crate::services::cohort::find_product_for_cohort_id(pool, cohort_id).await?;
    if can_manage_cohorts(pool, ctx, &product).await? {
        return Ok(());
    }
    let enrollment = sqlx::query_scalar!(r#"select id from enrollments where cohort_id = $1 and student_id = $2"#, cohort_id, ctx.user_id)
        .fetch_optional(pool)
        .await?;
    if enrollment.is_none() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

// POST /cohorts/{id}/class-sessions
pub async fn create_session(
    pool: &PgPool,
    meeting_provider: &dyn MeetingProvider,
    ctx: &AuthContext,
    cohort_id: Uuid,
    session_date: &str,
    scheduled_start: DateTime<Utc>,
    scheduled_end: DateTime<Utc>,
) -> Result<ClassSessionResponse, AppError> {
    assert_can_manage_cohort(pool, ctx, cohort_id).await?;
    if scheduled_end <= scheduled_start {
        return Err(AppError::UnprocessableEntity("invalid_schedule", "scheduled_end must be after scheduled_start".to_string()));
    }

    // Whichever provider this process was built with is used
    // unconditionally; meeting_provider.name() is written into the row
    // verbatim (not hardcoded) so simulate-participant's stub-only gate
    // stays correct even if the process's configured provider changes
    // later.
    let title = format!("class-session:{cohort_id}:{session_date}");
    let created = meeting_provider.create_meeting(&title, scheduled_start, scheduled_end).await?;

    let row = sqlx::query_as!(
        ClassSessionResponse,
        r#"insert into class_sessions (cohort_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url)
           values ($1, $2, $3, $4, $5, $6, $7)
           returning id, cohort_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url, status"#,
        cohort_id,
        session_date,
        scheduled_start,
        scheduled_end,
        meeting_provider.name(),
        created.external_meeting_id,
        created.join_url,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /cohorts/{id}/class-sessions
pub async fn list_sessions(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid) -> Result<ClassSessionListResponse, AppError> {
    assert_can_manage_cohort_or_enrolled(pool, ctx, cohort_id).await?;
    Ok(ClassSessionListResponse { items: list_by_cohort(pool, cohort_id).await? })
}

// GET /class-sessions/{id}
pub async fn get_session(pool: &PgPool, ctx: &AuthContext, class_session_id: Uuid) -> Result<ClassSessionResponse, AppError> {
    let session = find_by_id(pool, class_session_id).await?.ok_or(AppError::NotFound("class_session_not_found"))?;
    assert_can_manage_cohort_or_enrolled(pool, ctx, session.cohort_id).await?;
    Ok(session)
}

// GET /class-sessions/{id}/recording — the recording itself never
// touches this app's storage; this just proxies whether the provider
// has a Drive link yet.
pub async fn get_recording_status(
    pool: &PgPool,
    meeting_provider: &dyn MeetingProvider,
    ctx: &AuthContext,
    class_session_id: Uuid,
) -> Result<crate::services::meeting_provider::RecordingStatus, AppError> {
    let session = find_by_id(pool, class_session_id).await?.ok_or(AppError::NotFound("class_session_not_found"))?;
    assert_can_manage_cohort_or_enrolled(pool, ctx, session.cohort_id).await?;
    let Some(external_meeting_id) = &session.external_meeting_id else {
        return Ok(crate::services::meeting_provider::RecordingStatus { state: crate::services::meeting_provider::RecordingState::NotFound, drive_url: None });
    };
    meeting_provider.get_recording_status(external_meeting_id).await
}

#[derive(Debug, Clone)]
pub struct SessionParticipantRecord {
    pub id: Uuid,
    pub class_session_id: Uuid,
    pub user_id: Option<Uuid>,
    pub role: String,
    pub external_participant_name: Option<String>,
    pub external_google_account_id: Option<String>,
    pub first_joined_at: DateTime<Utc>,
    pub last_left_at: DateTime<Utc>,
    pub duration_seconds: i32,
    pub join_session_count: i32,
    pub verification_status: String,
}

async fn find_participant(pool: &PgPool, class_session_id: Uuid, role: &str, user_id: Option<Uuid>) -> Result<Option<SessionParticipantRecord>, AppError> {
    // user_id is nullable (unmatched participant) — matched in
    // application code, not a SQL predicate, since `= NULL` never
    // matches in SQL (mirrors the Bun repository's own deliberate
    // in-memory `.find()` for exactly this reason).
    let rows = sqlx::query_as!(
        SessionParticipantRecord,
        r#"select id, class_session_id, user_id, role, external_participant_name, external_google_account_id,
                  first_joined_at, last_left_at, duration_seconds, join_session_count, verification_status
           from session_participant_records where class_session_id = $1 and role = $2"#,
        class_session_id,
        role,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().find(|r| r.user_id == user_id))
}

// The simulate/sync paths both write here per-participant per sync run:
// a fresh sync for the SAME session should replace a participant's
// evidence row, not accumulate duplicates. NOT a single atomic `ON
// CONFLICT` — there is no unique constraint on (class_session_id, role,
// user_id) at the DB level, matching the Bun repository's own
// read-then-write emulation exactly (including its race-condition MVP
// limitation, not "fixed" here).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_participant(
    pool: &PgPool,
    class_session_id: Uuid,
    role: &str,
    user_id: Option<Uuid>,
    external_participant_name: Option<&str>,
    external_google_account_id: Option<&str>,
    first_joined_at: DateTime<Utc>,
    last_left_at: DateTime<Utc>,
    duration_seconds: i32,
    join_session_count: i32,
    verification_status: &str,
) -> Result<SessionParticipantRecord, AppError> {
    if let Some(existing) = find_participant(pool, class_session_id, role, user_id).await? {
        let row = sqlx::query_as!(
            SessionParticipantRecord,
            r#"update session_participant_records
               set external_participant_name = $2, external_google_account_id = $3, first_joined_at = $4,
                   last_left_at = $5, duration_seconds = $6, join_session_count = $7, verification_status = $8
               where id = $1
               returning id, class_session_id, user_id, role, external_participant_name, external_google_account_id,
                         first_joined_at, last_left_at, duration_seconds, join_session_count, verification_status"#,
            existing.id,
            external_participant_name,
            external_google_account_id,
            first_joined_at,
            last_left_at,
            duration_seconds,
            join_session_count,
            verification_status,
        )
        .fetch_one(pool)
        .await?;
        return Ok(row);
    }

    let row = sqlx::query_as!(
        SessionParticipantRecord,
        r#"insert into session_participant_records
             (class_session_id, role, user_id, external_participant_name, external_google_account_id,
              first_joined_at, last_left_at, duration_seconds, join_session_count, verification_status)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
           returning id, class_session_id, user_id, role, external_participant_name, external_google_account_id,
                     first_joined_at, last_left_at, duration_seconds, join_session_count, verification_status"#,
        class_session_id,
        role,
        user_id,
        external_participant_name,
        external_google_account_id,
        first_joined_at,
        last_left_at,
        duration_seconds,
        join_session_count,
        verification_status,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn list_participants(pool: &PgPool, class_session_id: Uuid) -> Result<Vec<SessionParticipantRecord>, AppError> {
    let rows = sqlx::query_as!(
        SessionParticipantRecord,
        r#"select id, class_session_id, user_id, role, external_participant_name, external_google_account_id,
                  first_joined_at, last_left_at, duration_seconds, join_session_count, verification_status
           from session_participant_records where class_session_id = $1"#,
        class_session_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
