use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::{exam_session, proctoring_policy};

// Port of proctoring_service.ts + proctoring_session_repository.ts +
// proctoring_event_repository.ts. P13-001..005. Kept as one file
// (Policy -> Event Collector -> Risk Engine -> Human Review -> Retention),
// mirroring the Bun original's own single-file structure.

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionResponse {
    pub id: Uuid,
    pub exam_session_id: Uuid,
    pub policy_id: Uuid,
    pub consent_given_at: DateTime<Utc>,
    pub review_status: String,
}

struct ProctoringSessionRow {
    id: Uuid,
    exam_session_id: Uuid,
    policy_id: Uuid,
    consent_given_at: DateTime<Utc>,
    review_status: String,
    reviewed_by: Option<Uuid>,
    reviewed_at: Option<DateTime<Utc>>,
    review_notes: Option<String>,
}

async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<ProctoringSessionRow>, AppError> {
    let row = sqlx::query_as!(
        ProctoringSessionRow,
        r#"select id, exam_session_id, policy_id, consent_given_at, review_status, reviewed_by, reviewed_at, review_notes
           from proctoring_sessions where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

fn to_session_response(row: &ProctoringSessionRow) -> SessionResponse {
    SessionResponse { id: row.id, exam_session_id: row.exam_session_id, policy_id: row.policy_id, consent_given_at: row.consent_given_at, review_status: row.review_status.clone() }
}

// Resolves the exam_session behind a proctoring_session id — does NOT
// itself verify ownership (each caller applies its own rule on top),
// matching the Bun original's loadOwnedProctoringSession naming despite
// not actually checking ownership.
async fn load_proctoring_session_and_exam_session(pool: &PgPool, proctoring_session_id: Uuid) -> Result<(ProctoringSessionRow, exam_session::ExamSessionRow), AppError> {
    let session = find_by_id(pool, proctoring_session_id).await?.ok_or(AppError::NotFound("proctoring_session_not_found"))?;
    let exam_sess = exam_session::find_by_id(pool, session.exam_session_id).await?.ok_or(AppError::NotFound("exam_session_not_found"))?;
    Ok((session, exam_sess))
}

// POST /exam-sessions/{id}/proctoring-session — consent-first: nothing
// is ever inserted without consent === true. There is no "session
// created but unconsented" state to clean up later.
pub async fn start_proctoring_session(pool: &PgPool, ctx: &AuthContext, exam_session_id: Uuid, policy_id: Uuid, consent: bool, device_info: Option<serde_json::Value>) -> Result<SessionResponse, AppError> {
    let exam_sess = exam_session::find_by_id(pool, exam_session_id).await?.ok_or(AppError::NotFound("exam_session_not_found"))?;
    if exam_sess.user_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }
    if !consent {
        return Err(AppError::UnprocessableEntity("consent_required", "explicit consent is required to start a proctored session".to_string()));
    }
    if proctoring_policy::find_by_id(pool, policy_id).await?.is_none() {
        return Err(AppError::NotFound("proctoring_policy_not_found"));
    }

    let row = sqlx::query_as!(
        ProctoringSessionRow,
        r#"insert into proctoring_sessions (exam_session_id, policy_id, device_info, consent_given_at)
           values ($1, $2, $3, now())
           returning id, exam_session_id, policy_id, consent_given_at, review_status, reviewed_by, reviewed_at, review_notes"#,
        exam_session_id,
        policy_id,
        device_info,
    )
    .fetch_one(pool)
    .await?;
    Ok(to_session_response(&row))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EventResponse {
    pub id: Uuid,
    pub proctoring_session_id: Uuid,
    pub r#type: String,
    pub severity: String,
    pub timestamp: DateTime<Utc>,
    pub evidence_id: Option<Uuid>,
}

const SEVERITIES: [&str; 3] = ["low", "medium", "high"];

// POST /proctoring-sessions/{id}/events — strict ownership only: only
// the exam session's own owner may report events, no staff bypass.
pub async fn record_event(pool: &PgPool, ctx: &AuthContext, proctoring_session_id: Uuid, r#type: &str, severity: &str, metadata: Option<serde_json::Value>, evidence_id: Option<Uuid>) -> Result<EventResponse, AppError> {
    let (session, exam_sess) = load_proctoring_session_and_exam_session(pool, proctoring_session_id).await?;
    if exam_sess.user_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }
    if !SEVERITIES.contains(&severity) {
        return Err(AppError::UnprocessableEntity("invalid_severity", "severity must be low, medium, or high".to_string()));
    }

    if let Some(evidence_id) = evidence_id {
        let asset = crate::services::asset::find_by_id(pool, evidence_id).await?;
        let owned = asset.map(|a| a.user_id == Some(ctx.user_id)).unwrap_or(false);
        if !owned {
            // Deliberately vague (422, not 403/404) so a student probing
            // other users' asset ids can't distinguish "doesn't exist"
            // from "not yours".
            return Err(AppError::UnprocessableEntity("invalid_evidence", "evidence_id must reference an asset you own".to_string()));
        }
    }

    let row = sqlx::query_as!(
        EventResponse,
        r#"insert into proctoring_events (proctoring_session_id, type, severity, metadata, evidence_id)
           values ($1, $2, $3, $4, $5)
           returning id, proctoring_session_id, type, severity, timestamp, evidence_id"#,
        session.id,
        r#type,
        severity,
        metadata,
        evidence_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

const SEVERITY_WEIGHT_LOW: i64 = 1;
const SEVERITY_WEIGHT_MEDIUM: i64 = 3;
const SEVERITY_WEIGHT_HIGH: i64 = 7;

// Pure function — the risk score is surfaced to a human reviewer and
// NEVER written anywhere or used to auto-decide anything. review_status
// only ever changes via the explicit review endpoint.
fn compute_risk_score(severities: &[String]) -> i64 {
    let total: i64 = severities
        .iter()
        .map(|s| match s.as_str() {
            "low" => SEVERITY_WEIGHT_LOW,
            "medium" => SEVERITY_WEIGHT_MEDIUM,
            "high" => SEVERITY_WEIGHT_HIGH,
            _ => 0,
        })
        .sum();
    total.min(100)
}

const MS_PER_DAY_SECONDS: i64 = 24 * 60 * 60;

// Lazy retention sweep (P13-005) — called only from get_session, no
// scheduler exists anywhere in this backend. Only severs the
// evidence_id link + stamps purged_at; the underlying asset row is
// never deleted (Drive/R9 owns asset lifecycle).
async fn purge_expired_evidence(pool: &PgPool, session: &ProctoringSessionRow) -> Result<(), AppError> {
    let Some(policy) = proctoring_policy::find_by_id(pool, session.policy_id).await? else { return Ok(()) };
    let cutoff = Utc::now() - chrono::Duration::seconds(policy.retention_days as i64 * MS_PER_DAY_SECONDS);
    sqlx::query!(
        r#"update proctoring_events set evidence_id = null, purged_at = now()
           where proctoring_session_id = $1 and evidence_id is not null and purged_at is null and "timestamp" <= $2"#,
        session.id,
        cutoff,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn is_staff(role: Option<&str>) -> bool {
    matches!(role, Some("platform_admin") | Some("org_owner") | Some("academic_director"))
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "view")]
#[serde(rename_all = "snake_case")]
pub enum SessionDetailResponse {
    Owner {
        id: Uuid,
        exam_session_id: Uuid,
        policy_id: Uuid,
        consent_given_at: DateTime<Utc>,
        review_status: String,
        reviewed_at: Option<DateTime<Utc>>,
    },
    Staff {
        id: Uuid,
        exam_session_id: Uuid,
        policy_id: Uuid,
        consent_given_at: DateTime<Utc>,
        review_status: String,
        reviewed_by: Option<Uuid>,
        reviewed_at: Option<DateTime<Utc>>,
        review_notes: Option<String>,
        risk_score: i64,
        events: Vec<EventResponse>,
    },
}

// GET /proctoring-sessions/{id}. Owner gets a reduced view (no events,
// no risk_score — "a student can't learn exactly what triggers
// detection and route around it next time"); staff gets the full
// packet. Note: this uses an inline role check, NOT require_permission
// — a deliberate structural difference from list_sessions_for_review/
// submit_review reproduced as-is (same 3 roles today, different code
// path).
pub async fn get_session(pool: &PgPool, ctx: &AuthContext, proctoring_session_id: Uuid) -> Result<SessionDetailResponse, AppError> {
    let (session, exam_sess) = load_proctoring_session_and_exam_session(pool, proctoring_session_id).await?;
    let is_owner = exam_sess.user_id == ctx.user_id;
    let staff = is_staff(ctx.role.as_deref());
    if !is_owner && !staff {
        return Err(AppError::Forbidden);
    }

    // Lazy purge on EVERY GET, for both owner and staff views.
    purge_expired_evidence(pool, &session).await?;
    let refreshed = find_by_id(pool, proctoring_session_id).await?.ok_or(AppError::NotFound("proctoring_session_not_found"))?;

    if !staff {
        return Ok(SessionDetailResponse::Owner {
            id: refreshed.id,
            exam_session_id: refreshed.exam_session_id,
            policy_id: refreshed.policy_id,
            consent_given_at: refreshed.consent_given_at,
            review_status: refreshed.review_status,
            reviewed_at: refreshed.reviewed_at,
        });
    }

    let events = sqlx::query_as!(
        EventResponse,
        r#"select id, proctoring_session_id, type, severity, timestamp, evidence_id from proctoring_events where proctoring_session_id = $1 order by timestamp asc"#,
        proctoring_session_id,
    )
    .fetch_all(pool)
    .await?;
    let severities: Vec<String> = events.iter().map(|e| e.severity.clone()).collect();
    let risk_score = compute_risk_score(&severities);

    Ok(SessionDetailResponse::Staff {
        id: refreshed.id,
        exam_session_id: refreshed.exam_session_id,
        policy_id: refreshed.policy_id,
        consent_given_at: refreshed.consent_given_at,
        review_status: refreshed.review_status,
        reviewed_by: refreshed.reviewed_by,
        reviewed_at: refreshed.reviewed_at,
        review_notes: refreshed.review_notes,
        risk_score,
        events,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct ReviewQueueRow {
    pub id: Uuid,
    pub exam_session_id: Uuid,
    pub student_name: String,
    pub assessment_title: String,
    pub review_status: String,
    pub consent_given_at: DateTime<Utc>,
    pub risk_score: i64,
}

// GET /proctoring-sessions (P20-001) — review-queue listing, unpaginated.
pub async fn list_sessions_for_review(pool: &PgPool, ctx: &AuthContext) -> Result<Vec<ReviewQueueRow>, AppError> {
    require_permission(ctx, Resource::ProctoringSession, Action::Review)?;

    struct Row {
        id: Uuid,
        exam_session_id: Uuid,
        student_name: String,
        assessment_title: String,
        review_status: String,
        consent_given_at: DateTime<Utc>,
    }
    let rows = sqlx::query_as!(
        Row,
        r#"select proctoring_sessions.id, proctoring_sessions.exam_session_id,
                  users.name as student_name, assessments.title as assessment_title,
                  proctoring_sessions.review_status, proctoring_sessions.consent_given_at
           from proctoring_sessions
           inner join exam_sessions on exam_sessions.id = proctoring_sessions.exam_session_id
           inner join users on users.id = exam_sessions.user_id
           inner join assessments on assessments.id = exam_sessions.assessment_id
           order by proctoring_sessions.consent_given_at desc"#,
    )
    .fetch_all(pool)
    .await?;

    // N+1: 1 events query per session — acceptable at this MVP's scale,
    // not silently hidden (matches the Bun original's own comment).
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let severities: Vec<String> = sqlx::query_scalar!(r#"select severity from proctoring_events where proctoring_session_id = $1"#, row.id).fetch_all(pool).await?;
        let risk_score = compute_risk_score(&severities);
        items.push(ReviewQueueRow { id: row.id, exam_session_id: row.exam_session_id, student_name: row.student_name, assessment_title: row.assessment_title, review_status: row.review_status, consent_given_at: row.consent_given_at, risk_score });
    }
    Ok(items)
}

const REVIEW_DECISIONS: [&str; 3] = ["cleared", "flagged", "violation_confirmed"];

// POST /proctoring-sessions/{id}/review — even the session's own
// student is forbidden (require_permission, no ownership fallback).
// Re-review is explicitly allowed: an internal moderation call that may
// need correction, unlike tutor_reviews' 409-on-repeat (a public rating
// whose integrity depends on a single final submission).
pub async fn submit_review(pool: &PgPool, ctx: &AuthContext, proctoring_session_id: Uuid, decision: &str, notes: Option<&str>) -> Result<SessionResponse, AppError> {
    require_permission(ctx, Resource::ProctoringSession, Action::Review)?;

    if !REVIEW_DECISIONS.contains(&decision) {
        return Err(AppError::UnprocessableEntity("invalid_decision", "decision must be cleared, flagged, or violation_confirmed".to_string()));
    }
    if find_by_id(pool, proctoring_session_id).await?.is_none() {
        return Err(AppError::NotFound("proctoring_session_not_found"));
    }

    let row = sqlx::query_as!(
        ProctoringSessionRow,
        r#"update proctoring_sessions set review_status = $2, reviewed_by = $3, reviewed_at = now(), review_notes = $4
           where id = $1
           returning id, exam_session_id, policy_id, consent_given_at, review_status, reviewed_by, reviewed_at, review_notes"#,
        proctoring_session_id,
        decision,
        ctx.user_id,
        notes,
    )
    .fetch_one(pool)
    .await?;
    Ok(to_session_response(&row))
}
