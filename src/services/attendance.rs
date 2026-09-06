use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::class_session::assert_can_manage_cohort;
use crate::services::cohort::can_manage_cohorts;

// P9-005, P28-001/002. Same "who may manage this cohort" rule as
// cohorts/enrollments (tutor owner OR org admin of that tutor's org)
// reused here for consistency.

const VALID_STATUSES: [&str; 4] = ["present", "absent", "late", "excused"];

pub struct AttendanceEntry {
    pub student_id: Uuid,
    pub status: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AttendanceRecordResponse {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub student_id: Uuid,
    pub session_date: String,
    pub status: String,
    pub method: String,
    // P28-001 — null for a system-verified row (method='online').
    pub marked_by: Option<Uuid>,
    pub marked_at: DateTime<Utc>,
    pub class_session_id: Option<Uuid>,
}

async fn find_one(pool: &PgPool, cohort_id: Uuid, student_id: Uuid, session_date: &str) -> Result<Option<AttendanceRecordResponse>, AppError> {
    let row = sqlx::query_as!(
        AttendanceRecordResponse,
        r#"select id, cohort_id, student_id, session_date, status, method, marked_by, marked_at, class_session_id
           from attendance_records where cohort_id = $1 and student_id = $2 and session_date = $3"#,
        cohort_id,
        student_id,
        session_date,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn upsert(pool: &PgPool, cohort_id: Uuid, student_id: Uuid, session_date: &str, status: &str, marked_by: Uuid) -> Result<AttendanceRecordResponse, AppError> {
    let row = sqlx::query_as!(
        AttendanceRecordResponse,
        r#"insert into attendance_records (cohort_id, student_id, session_date, status, marked_by)
           values ($1, $2, $3, $4, $5)
           on conflict (cohort_id, student_id, session_date) do update
             set status = excluded.status, marked_by = excluded.marked_by, marked_at = now()
           returning id, cohort_id, student_id, session_date, status, method, marked_by, marked_at, class_session_id"#,
        cohort_id,
        student_id,
        session_date,
        status,
        marked_by,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// P28-002 — same upsert shape as upsert() above, but for a
// system-verified row: method='online', marked_by=NULL (attributing a
// system-computed row to the tutor would misrepresent exactly what this
// feature exists to prevent), and class_session_id set.
async fn upsert_verified(pool: &PgPool, cohort_id: Uuid, student_id: Uuid, session_date: &str, status: &str, class_session_id: Uuid) -> Result<AttendanceRecordResponse, AppError> {
    let row = sqlx::query_as!(
        AttendanceRecordResponse,
        r#"insert into attendance_records (cohort_id, student_id, session_date, status, method, marked_by, class_session_id)
           values ($1, $2, $3, $4, 'online', null, $5)
           on conflict (cohort_id, student_id, session_date) do update
             set status = excluded.status, method = excluded.method, marked_by = excluded.marked_by,
                 class_session_id = excluded.class_session_id, marked_at = now()
           returning id, cohort_id, student_id, session_date, status, method, marked_by, marked_at, class_session_id"#,
        cohort_id,
        student_id,
        session_date,
        status,
        class_session_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn list_by_cohort(pool: &PgPool, cohort_id: Uuid) -> Result<Vec<AttendanceRecordResponse>, AppError> {
    let rows = sqlx::query_as!(
        AttendanceRecordResponse,
        r#"select id, cohort_id, student_id, session_date, status, method, marked_by, marked_at, class_session_id
           from attendance_records where cohort_id = $1"#,
        cohort_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn list_by_cohort_and_student(pool: &PgPool, cohort_id: Uuid, student_id: Uuid) -> Result<Vec<AttendanceRecordResponse>, AppError> {
    let rows = sqlx::query_as!(
        AttendanceRecordResponse,
        r#"select id, cohort_id, student_id, session_date, status, method, marked_by, marked_at, class_session_id
           from attendance_records where cohort_id = $1 and student_id = $2"#,
        cohort_id,
        student_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

enum WriteMode {
    Manual { marked_by: Uuid },
    Verified { class_session_id: Uuid },
}

// P23-002 — shared by both markAttendance (manual) and
// recordVerifiedAttendance (system-verified): only a genuine transition
// into/out of 'present' redeems/returns a package session. Re-marking
// the same date 'present' again must not double-charge; marking a date
// away from 'present' (correcting a mistake, or a verification
// downgrading it to 'partial') gives the session back. Only meaningful
// for a package enrollment (sessions_remaining is not null).
async fn apply_package_session_delta(pool: &PgPool, cohort_id: Uuid, student_id: Uuid, session_date: &str, next_status: &str, mode: WriteMode) -> Result<AttendanceRecordResponse, AppError> {
    let enrollment = sqlx::query!(r#"select id, sessions_remaining from enrollments where cohort_id = $1 and student_id = $2"#, cohort_id, student_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::UnprocessableEntity("student_not_enrolled", format!("student {student_id} is not enrolled in this cohort")))?;

    let prior_record = if enrollment.sessions_remaining.is_some() { find_one(pool, cohort_id, student_id, session_date).await? } else { None };
    let was_present = prior_record.map(|r| r.status == "present").unwrap_or(false);

    let record = match mode {
        WriteMode::Manual { marked_by } => upsert(pool, cohort_id, student_id, session_date, next_status, marked_by).await?,
        WriteMode::Verified { class_session_id } => upsert_verified(pool, cohort_id, student_id, session_date, next_status, class_session_id).await?,
    };
    let is_present = next_status == "present";

    if enrollment.sessions_remaining.is_some() {
        if !was_present && is_present {
            sqlx::query!(r#"update enrollments set sessions_remaining = sessions_remaining - 1 where id = $1"#, enrollment.id).execute(pool).await?;
        } else if was_present && !is_present {
            sqlx::query!(r#"update enrollments set sessions_remaining = sessions_remaining + 1 where id = $1"#, enrollment.id).execute(pool).await?;
        }
    }

    Ok(record)
}

// POST /cohorts/{id}/sessions/{session_date}/attendance
pub async fn mark_attendance(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid, session_date: &str, entries: Vec<AttendanceEntry>) -> Result<Vec<AttendanceRecordResponse>, AppError> {
    assert_can_manage_cohort(pool, ctx, cohort_id).await?;

    let mut results = Vec::with_capacity(entries.len());
    for entry in entries {
        if !VALID_STATUSES.contains(&entry.status.as_str()) {
            return Err(AppError::UnprocessableEntity("invalid_attendance_status", "status must be one of present/absent/late/excused".to_string()));
        }
        let record = apply_package_session_delta(pool, cohort_id, entry.student_id, session_date, &entry.status, WriteMode::Manual { marked_by: ctx.user_id }).await?;
        results.push(record);
    }
    Ok(results)
}

// Called only from attendance_verification::sync_attendance — no
// separate authorization check here (the caller already authorized the
// class_sessions/cohort access).
pub async fn record_verified_attendance(pool: &PgPool, cohort_id: Uuid, student_id: Uuid, session_date: &str, status: &str, class_session_id: Uuid) -> Result<AttendanceRecordResponse, AppError> {
    apply_package_session_delta(pool, cohort_id, student_id, session_date, status, WriteMode::Verified { class_session_id }).await
}

// GET /cohorts/{id}/attendance
pub async fn get_cohort_attendance(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid) -> Result<Vec<AttendanceRecordResponse>, AppError> {
    let (_cohort, product) = crate::services::cohort::find_product_for_cohort_id(pool, cohort_id).await?;

    if can_manage_cohorts(pool, ctx, &product).await? {
        return list_by_cohort(pool, cohort_id).await;
    }

    let enrollment = sqlx::query_scalar!(r#"select id from enrollments where cohort_id = $1 and student_id = $2"#, cohort_id, ctx.user_id)
        .fetch_optional(pool)
        .await?;
    if enrollment.is_some() {
        return list_by_cohort_and_student(pool, cohort_id, ctx.user_id).await;
    }
    Err(AppError::Forbidden)
}
