use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::org_class::assert_can_manage_class;

// Phase 35 (M2) — manual attendance marking for the org-owned `classes`
// domain. Mirrors services/attendance.rs's shape, minus the
// marketplace's package-session-delta billing logic (no
// enrollments/sessions_remaining concept here — classes are a plain
// roster, not a paid product). Status vocabulary is the Indonesian
// school-attendance set the user actually asked for, not the
// marketplace's present/absent/late/excused — see migration header.

const VALID_STATUSES: [&str; 5] = ["hadir", "alpha", "sakit", "izin", "telat"];

pub struct AttendanceEntry {
    pub student_id: Uuid,
    pub status: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OrgAttendanceRecordResponse {
    pub id: Uuid,
    pub class_id: Uuid,
    pub student_id: Uuid,
    pub session_date: NaiveDate,
    pub status: String,
    pub method: String,
    // Null for a system-verified row (method='google_meet') — the same
    // reasoning as the marketplace's own attendance.rs: attributing a
    // system-computed row to a person would misrepresent it.
    pub marked_by: Option<Uuid>,
    pub marked_at: DateTime<Utc>,
    pub class_session_id: Option<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct AttendanceRosterRow {
    pub student_id: Uuid,
    pub name: String,
    pub email: String,
    // None means "belum diabsen" — deliberately not a status value,
    // just the absence of a row for this student+date.
    pub status: Option<String>,
    pub method: Option<String>,
}

async fn is_class_member(pool: &PgPool, class_id: Uuid, student_id: Uuid) -> Result<bool, AppError> {
    let row = sqlx::query_scalar!(
        r#"select 1 as "exists!" from class_members where class_id = $1 and student_id = $2"#,
        class_id,
        student_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

pub(crate) async fn upsert(
    pool: &PgPool,
    class_id: Uuid,
    student_id: Uuid,
    session_date: NaiveDate,
    status: &str,
    method: &str,
    marked_by: Option<Uuid>,
    class_session_id: Option<Uuid>,
) -> Result<OrgAttendanceRecordResponse, AppError> {
    let row = sqlx::query_as!(
        OrgAttendanceRecordResponse,
        r#"insert into org_attendance_records (class_id, student_id, session_date, status, method, marked_by, class_session_id)
           values ($1, $2, $3, $4, $5, $6, $7)
           on conflict (class_id, student_id, session_date) do update
             set status = excluded.status, method = excluded.method, marked_by = excluded.marked_by,
                 class_session_id = excluded.class_session_id, marked_at = now()
           returning id, class_id, student_id, session_date, status, method, marked_by, marked_at, class_session_id"#,
        class_id,
        student_id,
        session_date,
        status,
        method,
        marked_by,
        class_session_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// POST /classes/{id}/attendance/{date}
pub async fn mark_attendance(pool: &PgPool, ctx: &AuthContext, class_id: Uuid, session_date: NaiveDate, entries: Vec<AttendanceEntry>) -> Result<Vec<OrgAttendanceRecordResponse>, AppError> {
    assert_can_manage_class(pool, ctx, class_id).await?;

    let mut results = Vec::with_capacity(entries.len());
    for entry in entries {
        if !VALID_STATUSES.contains(&entry.status.as_str()) {
            return Err(AppError::UnprocessableEntity("invalid_attendance_status", "status must be one of hadir/alpha/sakit/izin/telat".to_string()));
        }
        if !is_class_member(pool, class_id, entry.student_id).await? {
            return Err(AppError::UnprocessableEntity("student_not_in_class", format!("student {} is not a member of this class", entry.student_id)));
        }
        let record = upsert(pool, class_id, entry.student_id, session_date, &entry.status, "manual", Some(ctx.user_id), None).await?;
        results.push(record);
    }
    Ok(results)
}

// Shared by the QR teacher-scan and QR student-self-check-in paths
// (handlers/org_attendance.rs) — both always mark 'hadir' (scanning IS
// the presence signal), only `method`/`marked_by` differ.
pub(crate) async fn record_qr_attendance(pool: &PgPool, class_id: Uuid, student_id: Uuid, session_date: NaiveDate, method: &str, marked_by: Uuid, class_session_id: Uuid) -> Result<OrgAttendanceRecordResponse, AppError> {
    upsert(pool, class_id, student_id, session_date, "hadir", method, Some(marked_by), Some(class_session_id)).await
}

// Called only from org_attendance_verification::sync_attendance — no
// separate authorization here (the caller already authorized session
// access).
pub(crate) async fn record_verified_attendance(pool: &PgPool, class_id: Uuid, student_id: Uuid, session_date: NaiveDate, status: &str, class_session_id: Uuid) -> Result<OrgAttendanceRecordResponse, AppError> {
    upsert(pool, class_id, student_id, session_date, status, "google_meet", None, Some(class_session_id)).await
}

// GET /classes/{id}/attendance/{date} — the whole roster for one date,
// LEFT JOINed against any existing record so an unmarked student comes
// back with status=None ("belum diabsen") instead of being omitted.
pub async fn get_roster_for_date(pool: &PgPool, ctx: &AuthContext, class_id: Uuid, session_date: NaiveDate) -> Result<Vec<AttendanceRosterRow>, AppError> {
    assert_can_manage_class(pool, ctx, class_id).await?;
    let rows = sqlx::query_as!(
        AttendanceRosterRow,
        r#"select u.id as "student_id!", u.name as "name!", u.email as "email!", ar.status as "status?", ar.method as "method?"
           from class_members cm
           inner join users u on u.id = cm.student_id
           left join org_attendance_records ar
             on ar.class_id = cm.class_id and ar.student_id = cm.student_id and ar.session_date = $2
           where cm.class_id = $1
           order by u.name asc"#,
        class_id,
        session_date,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
