use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::meeting_provider::MeetingProvider;
use crate::services::org_class::{assert_can_manage_class, assert_is_member_or_manager};

// Phase 35 (M1) — session scheduling for the org-owned `classes` domain,
// mirroring services/class_session.rs's shape exactly (same
// MeetingProvider trait, same create/list/status pattern) but FK'd to
// `classes` and gated through org_class.rs instead of
// cohort.product_id -> learning_products.tutor_id. See
// migrations/0035_org_class_sessions_attendance.sql's header for why
// this is a new table rather than a retrofit.

#[derive(Debug, Clone, serde::Serialize)]
pub struct OrgClassSessionResponse {
    pub id: Uuid,
    pub class_id: Uuid,
    pub session_date: NaiveDate,
    pub scheduled_start: DateTime<Utc>,
    pub scheduled_end: DateTime<Utc>,
    pub meeting_provider: String,
    pub external_meeting_id: Option<String>,
    pub join_url: Option<String>,
    pub status: String,
}

#[derive(Debug, serde::Serialize)]
pub struct OrgClassSessionListResponse {
    pub items: Vec<OrgClassSessionResponse>,
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<OrgClassSessionResponse>, AppError> {
    let row = sqlx::query_as!(
        OrgClassSessionResponse,
        r#"select id, class_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url, status
           from org_class_sessions where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn list_by_class(pool: &PgPool, class_id: Uuid) -> Result<Vec<OrgClassSessionResponse>, AppError> {
    let rows = sqlx::query_as!(
        OrgClassSessionResponse,
        r#"select id, class_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url, status
           from org_class_sessions where class_id = $1 order by scheduled_start desc"#,
        class_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> Result<OrgClassSessionResponse, AppError> {
    let row = sqlx::query_as!(
        OrgClassSessionResponse,
        r#"update org_class_sessions set status = $2 where id = $1
           returning id, class_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url, status"#,
        id,
        status,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// POST /classes/{id}/sessions
pub async fn create_session(
    pool: &PgPool,
    meeting_provider: &dyn MeetingProvider,
    ctx: &AuthContext,
    class_id: Uuid,
    session_date: NaiveDate,
    scheduled_start: DateTime<Utc>,
    scheduled_end: DateTime<Utc>,
) -> Result<OrgClassSessionResponse, AppError> {
    assert_can_manage_class(pool, ctx, class_id).await?;
    if scheduled_end <= scheduled_start {
        return Err(AppError::UnprocessableEntity("invalid_schedule", "scheduled_end must be after scheduled_start".to_string()));
    }

    let title = format!("org-class-session:{class_id}:{session_date}");
    let created = meeting_provider.create_meeting(&title, scheduled_start, scheduled_end).await?;

    let row = sqlx::query_as!(
        OrgClassSessionResponse,
        r#"insert into org_class_sessions (class_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url)
           values ($1, $2, $3, $4, $5, $6, $7)
           returning id, class_id, session_date, scheduled_start, scheduled_end, meeting_provider, external_meeting_id, join_url, status"#,
        class_id,
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

// GET /classes/{id}/sessions — any class member (teacher, org-admin
// tier, or a plain enrolled student) can see the schedule; only
// managing it (create/sync/mark) is restricted.
pub async fn list_sessions(pool: &PgPool, ctx: &AuthContext, class_id: Uuid) -> Result<OrgClassSessionListResponse, AppError> {
    assert_is_member_or_manager(pool, ctx, class_id).await?;
    Ok(OrgClassSessionListResponse { items: list_by_class(pool, class_id).await? })
}

// GET /org-class-sessions/{id}
pub async fn get_session(pool: &PgPool, ctx: &AuthContext, class_session_id: Uuid) -> Result<OrgClassSessionResponse, AppError> {
    let session = find_by_id(pool, class_session_id).await?.ok_or(AppError::NotFound("class_session_not_found"))?;
    assert_is_member_or_manager(pool, ctx, session.class_id).await?;
    Ok(session)
}

// Phase 35 (M4) — Google Meet join/leave evidence per session, mirroring
// class_session.rs's own SessionParticipantRecord/upsert_participant
// exactly (same "no unique constraint, read-then-write" MVP shape).
#[derive(Debug, Clone)]
pub struct OrgSessionParticipantRecord {
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

async fn find_participant(pool: &PgPool, class_session_id: Uuid, role: &str, user_id: Option<Uuid>) -> Result<Option<OrgSessionParticipantRecord>, AppError> {
    let rows = sqlx::query_as!(
        OrgSessionParticipantRecord,
        r#"select id, class_session_id, user_id, role, external_participant_name, external_google_account_id,
                  first_joined_at, last_left_at, duration_seconds, join_session_count, verification_status
           from org_session_participant_records where class_session_id = $1 and role = $2"#,
        class_session_id,
        role,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().find(|r| r.user_id == user_id))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn upsert_participant(
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
) -> Result<OrgSessionParticipantRecord, AppError> {
    if let Some(existing) = find_participant(pool, class_session_id, role, user_id).await? {
        let row = sqlx::query_as!(
            OrgSessionParticipantRecord,
            r#"update org_session_participant_records
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
        OrgSessionParticipantRecord,
        r#"insert into org_session_participant_records
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

pub(crate) async fn list_participants(pool: &PgPool, class_session_id: Uuid) -> Result<Vec<OrgSessionParticipantRecord>, AppError> {
    let rows = sqlx::query_as!(
        OrgSessionParticipantRecord,
        r#"select id, class_session_id, user_id, role, external_participant_name, external_google_account_id,
                  first_joined_at, last_left_at, duration_seconds, join_session_count, verification_status
           from org_session_participant_records where class_session_id = $1"#,
        class_session_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
