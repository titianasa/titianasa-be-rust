use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::attendance_verification::compute_verification_status;
use crate::services::meeting_provider::MeetingProvider;
use crate::services::org_class::assert_can_manage_class;
use crate::services::org_class_session::{self, OrgClassSessionResponse};

// Phase 35 (M4) — Google Meet auto-detected attendance, ported from
// attendance_verification.rs's sync_attendance pipeline (the identical
// pure compute_verification_status is reused directly, not
// re-implemented) but retargeted at org_class_sessions/classes instead
// of cohorts, with no package-billing side effect (org classes aren't a
// paid product). The marketplace's present/late/partial/absent
// verification statuses are mapped onto this domain's Indonesian
// hadir/telat/izin/alpha vocabulary — 'partial' -> 'izin' is a genuine
// judgment call (attended some but not enough to count as full
// presence; closer to an excused partial attendance than an unexcused
// absence), not a 1:1 rename.
fn map_verification_status(status: &str) -> &'static str {
    match status {
        "present" => "hadir",
        "late" => "telat",
        "partial" => "izin",
        _ => "alpha",
    }
}

async fn authorize_session(pool: &PgPool, ctx: &AuthContext, class_session_id: Uuid) -> Result<OrgClassSessionResponse, AppError> {
    let session = org_class_session::find_by_id(pool, class_session_id).await?.ok_or(AppError::NotFound("class_session_not_found"))?;
    assert_can_manage_class(pool, ctx, session.class_id).await?;
    Ok(session)
}

async fn sync_from_provider(pool: &PgPool, config: &Config, meeting_provider: &dyn MeetingProvider, session: &OrgClassSessionResponse, teacher_id: Uuid) -> Result<(), AppError> {
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
        // Role inferred by identity match against the class's teacher,
        // never trusted from the provider — an unmatched participant is
        // recorded as 'student', the least-consequential default.
        let role = if matched_user == Some(teacher_id) { "teacher" } else { "student" };
        let status = compute_verification_status(session.scheduled_start, session.scheduled_end, entry.first_joined_at, entry.duration_seconds, config);
        org_class_session::upsert_participant(
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

#[derive(Debug, serde::Serialize)]
pub struct SyncOrgAttendanceResult {
    pub session: OrgClassSessionResponse,
    pub student_records: i64,
}

// POST /org-class-sessions/{id}/sync-attendance — idempotent, same as
// the marketplace's.
pub async fn sync_attendance(pool: &PgPool, config: &Config, meeting_provider: &dyn MeetingProvider, ctx: &AuthContext, class_session_id: Uuid) -> Result<SyncOrgAttendanceResult, AppError> {
    let session = authorize_session(pool, ctx, class_session_id).await?;
    let class = crate::services::org_class::find_by_id(pool, session.class_id).await?.ok_or(AppError::NotFound("class_not_found"))?;

    sync_from_provider(pool, config, meeting_provider, &session, class.teacher_id).await?;

    let participants = org_class_session::list_participants(pool, class_session_id).await?;
    let roster_student_ids: std::collections::HashSet<Uuid> =
        sqlx::query_scalar!(r#"select student_id from class_members where class_id = $1"#, session.class_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    let mut student_records = 0i64;
    for participant in &participants {
        let recomputed = compute_verification_status(session.scheduled_start, session.scheduled_end, participant.first_joined_at, participant.duration_seconds, config);
        org_class_session::upsert_participant(
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
                    let status = map_verification_status(recomputed);
                    crate::services::org_attendance::record_verified_attendance(pool, session.class_id, user_id, session.session_date, status, class_session_id).await?;
                    student_records += 1;
                }
            }
        }
    }

    // Any class member with NO participant evidence at all is alpha —
    // written explicitly rather than left as a silent gap.
    let seen_student_ids: std::collections::HashSet<Uuid> = participants.iter().filter(|p| p.role == "student").filter_map(|p| p.user_id).collect();
    for student_id in &roster_student_ids {
        if seen_student_ids.contains(student_id) {
            continue;
        }
        crate::services::org_attendance::record_verified_attendance(pool, session.class_id, *student_id, session.session_date, "alpha", class_session_id).await?;
        student_records += 1;
    }

    let updated = org_class_session::set_status(pool, class_session_id, "completed").await?;
    Ok(SyncOrgAttendanceResult { session: updated, student_records })
}
