use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::class_session::{self, ClassSessionResponse, SessionParticipantRecord};
use crate::services::cohort::can_manage_cohorts;
use crate::services::meeting_provider::MeetingProvider;

// P28-002 — pure function, no DB access, trivially testable against
// fixed timestamps. Never called with 0 scheduled duration
// (class_session::create_session validates scheduled_end > scheduled_start
// at creation) so the ratio division is always well-defined.
pub fn compute_verification_status(scheduled_start: DateTime<Utc>, scheduled_end: DateTime<Utc>, first_joined_at: DateTime<Utc>, duration_seconds: i32, config: &Config) -> &'static str {
    let scheduled_seconds = (scheduled_end - scheduled_start).num_seconds() as f64;
    let duration_ratio = duration_seconds as f64 / scheduled_seconds;
    if duration_ratio < config.attendance_min_duration_ratio {
        return "partial";
    }

    let late_minutes = (first_joined_at - scheduled_start).num_seconds() as f64 / 60.0;
    if late_minutes > config.attendance_late_join_minutes as f64 {
        return "late";
    }
    "present"
}

struct SessionAuth {
    session: ClassSessionResponse,
    cohort_id: Uuid,
    tutor_id: Uuid,
    is_manager: bool,
}

async fn authorize_session(pool: &PgPool, ctx: &AuthContext, class_session_id: Uuid) -> Result<SessionAuth, AppError> {
    let session = class_session::find_by_id(pool, class_session_id).await?.ok_or(AppError::NotFound("class_session_not_found"))?;
    let (_cohort, product) = crate::services::cohort::find_product_for_cohort_id(pool, session.cohort_id).await?;

    let is_manager = can_manage_cohorts(pool, ctx, &product).await?;
    if is_manager {
        return Ok(SessionAuth { cohort_id: session.cohort_id, tutor_id: product.tutor_id, session, is_manager: true });
    }

    let enrollment = sqlx::query_scalar!(r#"select id from enrollments where cohort_id = $1 and student_id = $2"#, session.cohort_id, ctx.user_id)
        .fetch_optional(pool)
        .await?;
    if enrollment.is_none() {
        return Err(AppError::Forbidden);
    }
    Ok(SessionAuth { cohort_id: session.cohort_id, tutor_id: product.tutor_id, session, is_manager: false })
}

pub struct SimulateParticipantParams {
    pub role: String,
    pub user_id: Uuid,
    pub join_offset_minutes: i64,
    pub duration_minutes: i64,
}

// POST /class-sessions/{id}/simulate-participant — dev-only, and only
// for sessions actually using the stub provider: rejecting this on a
// google_meet/zoom session means it can never be used to fabricate
// evidence on what would be a real, provider-verified session.
pub async fn simulate_participant(pool: &PgPool, config: &Config, ctx: &AuthContext, class_session_id: Uuid, params: SimulateParticipantParams) -> Result<SessionParticipantRecord, AppError> {
    let auth = authorize_session(pool, ctx, class_session_id).await?;
    if !auth.is_manager {
        return Err(AppError::Forbidden);
    }
    if auth.session.meeting_provider != "stub" {
        return Err(AppError::UnprocessableEntity("not_a_stub_session", "simulate-participant is only available on meeting_provider='stub' sessions".to_string()));
    }
    if params.role == "tutor" && params.user_id != auth.tutor_id {
        return Err(AppError::UnprocessableEntity("role_mismatch", "role='tutor' must be the cohort's actual tutor".to_string()));
    }

    let first_joined_at = auth.session.scheduled_start + chrono::Duration::minutes(params.join_offset_minutes);
    let duration_seconds = (params.duration_minutes * 60) as i32;
    let last_left_at = first_joined_at + chrono::Duration::seconds(duration_seconds as i64);
    let user = sqlx::query!(r#"select name, google_id from users where id = $1"#, params.user_id).fetch_optional(pool).await?;
    let status = compute_verification_status(auth.session.scheduled_start, auth.session.scheduled_end, first_joined_at, duration_seconds, config);

    class_session::upsert_participant(
        pool,
        class_session_id,
        &params.role,
        Some(params.user_id),
        user.as_ref().map(|u| u.name.as_str()),
        user.as_ref().and_then(|u| u.google_id.as_deref()),
        first_joined_at,
        last_left_at,
        duration_seconds,
        1,
        status,
    )
    .await
}

// For a real provider (meeting_provider != 'stub'), pulls the provider's
// participant report and upserts it into session_participant_records
// BEFORE the status-computation pass reads it back — the stub path
// skips this entirely (it's already fed directly by
// simulate_participant). Role is inferred by identity match against the
// cohort's tutor, never trusted from the provider — an unmatched
// participant is recorded with role='student' as the
// least-consequential default, never letting them masquerade as tutor.
async fn sync_from_provider(pool: &PgPool, config: &Config, meeting_provider: &dyn MeetingProvider, session: &ClassSessionResponse, tutor_id: Uuid) -> Result<(), AppError> {
    let Some(external_meeting_id) = &session.external_meeting_id else { return Ok(()) };
    if session.meeting_provider == "stub" {
        return Ok(());
    }

    let report = meeting_provider.get_participant_report(external_meeting_id).await?;
    for entry in report {
        let matched_user = match &entry.external_google_account_id {
            Some(google_id) => sqlx::query_scalar!(r#"select id from users where google_id = $1"#, google_id).fetch_optional(pool).await?,
            None => None,
        };
        let role = if matched_user == Some(tutor_id) { "tutor" } else { "student" };
        let status = compute_verification_status(session.scheduled_start, session.scheduled_end, entry.first_joined_at, entry.duration_seconds, config);
        class_session::upsert_participant(
            pool,
            session.id,
            role,
            matched_user,
            entry.external_participant_name.as_deref(),
            entry.external_google_account_id.as_deref(),
            entry.first_joined_at,
            entry.last_left_at,
            entry.duration_seconds,
            entry.join_session_count,
            status,
        )
        .await?;
    }
    Ok(())
}

pub struct SyncAttendanceResult {
    pub session: ClassSessionResponse,
    pub student_records: i64,
}

// POST /class-sessions/{id}/sync-attendance — idempotent: re-running it
// recomputes from the same evidence and upserts the same derived rows,
// never accumulates duplicates.
pub async fn sync_attendance(pool: &PgPool, config: &Config, meeting_provider: &dyn MeetingProvider, ctx: &AuthContext, class_session_id: Uuid) -> Result<SyncAttendanceResult, AppError> {
    let auth = authorize_session(pool, ctx, class_session_id).await?;
    if !auth.is_manager {
        return Err(AppError::Forbidden);
    }

    sync_from_provider(pool, config, meeting_provider, &auth.session, auth.tutor_id).await?;

    let participants = class_session::list_participants(pool, class_session_id).await?;
    let enrollments = sqlx::query!(r#"select student_id, status from enrollments where cohort_id = $1"#, auth.cohort_id).fetch_all(pool).await?;
    // Not filtered to status='active' — manual marking never makes that
    // distinction either, it only checks an enrollment row exists at
    // all. 'cancelled' is the only status that genuinely means "not in
    // this class" for verification purposes.
    let roster_student_ids: std::collections::HashSet<Uuid> = enrollments.iter().filter(|e| e.status != "cancelled").map(|e| e.student_id).collect();

    let mut student_records = 0i64;
    for participant in &participants {
        let recomputed = compute_verification_status(auth.session.scheduled_start, auth.session.scheduled_end, participant.first_joined_at, participant.duration_seconds, config);
        class_session::upsert_participant(
            pool,
            class_session_id,
            &participant.role,
            participant.user_id,
            participant.external_participant_name.as_deref(),
            participant.external_google_account_id.as_deref(),
            participant.first_joined_at,
            participant.last_left_at,
            participant.duration_seconds,
            participant.join_session_count,
            recomputed,
        )
        .await?;

        if participant.role == "student" {
            if let Some(user_id) = participant.user_id {
                if roster_student_ids.contains(&user_id) {
                    crate::services::attendance::record_verified_attendance(pool, auth.cohort_id, user_id, &auth.session.session_date, recomputed, class_session_id).await?;
                    student_records += 1;
                }
            }
        }
    }

    // Any enrolled (non-cancelled) student with NO participant evidence
    // at all is absent — written explicitly rather than left as a
    // silent gap.
    let seen_student_ids: std::collections::HashSet<Uuid> = participants.iter().filter(|p| p.role == "student").filter_map(|p| p.user_id).collect();
    for student_id in &roster_student_ids {
        if seen_student_ids.contains(student_id) {
            continue;
        }
        crate::services::attendance::record_verified_attendance(pool, auth.cohort_id, *student_id, &auth.session.session_date, "absent", class_session_id).await?;
        student_records += 1;
    }

    let updated = class_session::set_status(pool, class_session_id, "completed").await?;
    Ok(SyncAttendanceResult { session: updated, student_records })
}

#[derive(Debug, serde::Serialize)]
pub struct SessionAttendanceRow {
    pub role: String,
    pub user_id: Uuid,
    pub name: Option<String>,
    pub matched: bool,
    pub first_joined_at: Option<DateTime<Utc>>,
    pub last_left_at: Option<DateTime<Utc>>,
    pub duration_seconds: i32,
    pub verification_status: String,
}

async fn row_for(pool: &PgPool, participants: &[SessionParticipantRecord], role: &str, user_id: Uuid) -> Result<SessionAttendanceRow, AppError> {
    let p = participants.iter().find(|p| p.role == role && p.user_id == Some(user_id));
    let user = sqlx::query!(r#"select name from users where id = $1"#, user_id).fetch_optional(pool).await?;
    Ok(SessionAttendanceRow {
        role: role.to_string(),
        user_id,
        name: user.map(|u| u.name),
        matched: p.map(|p| p.user_id.is_some()).unwrap_or(false),
        first_joined_at: p.map(|p| p.first_joined_at),
        last_left_at: p.map(|p| p.last_left_at),
        duration_seconds: p.map(|p| p.duration_seconds).unwrap_or(0),
        verification_status: p.map(|p| p.verification_status.clone()).unwrap_or_else(|| "absent".to_string()),
    })
}

// GET /class-sessions/{id}/attendance — a manager sees the full roster
// (tutor + every non-cancelled-enrollment student); a plain enrolled
// student sees only the tutor's row and their own.
pub async fn get_session_attendance_report(pool: &PgPool, ctx: &AuthContext, class_session_id: Uuid) -> Result<Vec<SessionAttendanceRow>, AppError> {
    let auth = authorize_session(pool, ctx, class_session_id).await?;
    let participants = class_session::list_participants(pool, class_session_id).await?;

    let tutor_row = row_for(pool, &participants, "tutor", auth.tutor_id).await?;
    if !auth.is_manager {
        let own_row = row_for(pool, &participants, "student", ctx.user_id).await?;
        return Ok(vec![tutor_row, own_row]);
    }

    let enrollments = sqlx::query!(r#"select student_id, status from enrollments where cohort_id = $1"#, auth.cohort_id).fetch_all(pool).await?;
    let roster_student_ids: Vec<Uuid> = enrollments.into_iter().filter(|e| e.status != "cancelled").map(|e| e.student_id).collect();

    let mut rows = vec![tutor_row];
    for student_id in roster_student_ids {
        rows.push(row_for(pool, &participants, "student", student_id).await?);
    }
    Ok(rows)
}
