use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{NaiveDate, Utc};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::org_attendance::{MarkOrgAttendanceRequest, ScanAttendanceRequest};
use crate::services::{attendance_qr, org_attendance, org_class, org_class_session};
use crate::state::AppState;

/// "Attendance Guard": a check-in is refused until the guarded items in
/// the class's modules are finished, with the reasons spelled out.
async fn ensure_guards_met(state: &AppState, class_id: Uuid, student_id: Uuid) -> Result<(), AppError> {
    let blockers = crate::services::item_guard::attendance_blockers(&state.db, student_id, class_id).await?;
    if blockers.is_empty() {
        return Ok(());
    }
    let reasons = blockers
        .iter()
        .map(|b| {
            b.message.clone().unwrap_or_else(|| {
                let scope = if b.guard_type == "section_complete" { " beserta materi lain di foldernya" } else { "" };
                format!("Selesaikan \"{}\"{scope} sebelum check-in.", b.module_item_title)
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    Err(AppError::UnprocessableEntity("attendance_guard_blocked", reasons))
}

fn parse_date(value: &str) -> Result<NaiveDate, AppError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| AppError::UnprocessableEntity("invalid_date", "date must be YYYY-MM-DD".to_string()))
}

// GET /classes/{id}/attendance/{date}
pub async fn get_attendance(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((class_id, date)): Path<(Uuid, String)>,
) -> Result<Json<Vec<org_attendance::AttendanceRosterRow>>, AppError> {
    let session_date = parse_date(&date)?;
    Ok(Json(org_attendance::get_roster_for_date(&state.db, &ctx, class_id, session_date).await?))
}

// POST /classes/{id}/attendance/{date} — manual marking.
pub async fn post_attendance(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((class_id, date)): Path<(Uuid, String)>,
    ValidatedJson(body): ValidatedJson<MarkOrgAttendanceRequest>,
) -> Result<Json<Vec<org_attendance::OrgAttendanceRecordResponse>>, AppError> {
    let session_date = parse_date(&date)?;
    let entries = body.records.into_iter().map(|r| org_attendance::AttendanceEntry { student_id: r.student_id, status: r.status }).collect();
    Ok(Json(org_attendance::mark_attendance(&state.db, &ctx, class_id, session_date, entries).await?))
}

// GET /me/attendance-qr-token — a student's own durable "attendance
// card" payload, rendered as a QR on their Profil page.
pub async fn get_my_qr_token(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<serde_json::Value>, AppError> {
    let token = attendance_qr::issue_student_token(&state.config.jwt_access_secret, ctx.user_id)?;
    Ok(Json(serde_json::json!({ "token": token })))
}

// POST /org-class-sessions/{id}/attendance/scan — teacher scans a
// student's QR code during that session.
pub async fn post_scan(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<ScanAttendanceRequest>,
) -> Result<(StatusCode, Json<org_attendance::OrgAttendanceRecordResponse>), AppError> {
    let session = org_class_session::find_by_id(&state.db, session_id).await?.ok_or(AppError::NotFound("class_session_not_found"))?;
    org_class::assert_can_manage_class(&state.db, &ctx, session.class_id).await?;

    let student_id = attendance_qr::verify_student_token(&body.token, &state.config.jwt_access_secret)?;
    let is_member = sqlx::query_scalar!(r#"select 1 as "exists!" from class_members where class_id = $1 and student_id = $2"#, session.class_id, student_id)
        .fetch_optional(&state.db)
        .await?;
    if is_member.is_none() {
        return Err(AppError::UnprocessableEntity("student_not_in_class", "siswa ini bukan anggota kelas ini".to_string()));
    }

    ensure_guards_met(&state, session.class_id, student_id).await?;
    let record = org_attendance::record_qr_attendance(&state.db, session.class_id, student_id, session.session_date, "qr_teacher", ctx.user_id, session_id).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

// POST /org-class-sessions/{id}/attendance/self-check-in — a student
// scans that day's session QR (the session's own id) to check
// themselves in. Only valid for TODAY's still-open session — that check
// is what actually prevents replaying a stale/screenshotted code, not
// the QR payload itself (the session id is already an unguessable
// capability).
pub async fn post_self_check_in(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
) -> Result<(StatusCode, Json<org_attendance::OrgAttendanceRecordResponse>), AppError> {
    let session = org_class_session::find_by_id(&state.db, session_id).await?.ok_or(AppError::NotFound("class_session_not_found"))?;
    org_class::assert_is_member_or_manager(&state.db, &ctx, session.class_id).await?;

    if session.status != "scheduled" {
        return Err(AppError::UnprocessableEntity("session_not_open", "sesi ini sudah tidak menerima check-in".to_string()));
    }
    if session.session_date != Utc::now().date_naive() {
        return Err(AppError::UnprocessableEntity("session_not_today", "kode QR sesi ini hanya berlaku pada tanggalnya sendiri".to_string()));
    }

    let is_member = sqlx::query_scalar!(r#"select 1 as "exists!" from class_members where class_id = $1 and student_id = $2"#, session.class_id, ctx.user_id)
        .fetch_optional(&state.db)
        .await?;
    if is_member.is_none() {
        return Err(AppError::UnprocessableEntity("student_not_in_class", "kamu bukan anggota kelas ini".to_string()));
    }

    ensure_guards_met(&state, session.class_id, ctx.user_id).await?;
    let record = org_attendance::record_qr_attendance(&state.db, session.class_id, ctx.user_id, session.session_date, "qr_student", ctx.user_id, session_id).await?;
    Ok((StatusCode::CREATED, Json(record)))
}
