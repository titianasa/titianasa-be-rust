use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};

// Port of proctoring_policy_service.ts + proctoring_policy_repository.ts.
// P13-001. Policies are flat, reusable config rows — not org-scoped, not
// tied to a specific assessment.

#[derive(Debug, Clone, serde::Serialize)]
pub struct PolicyResponse {
    pub id: Uuid,
    pub exam_type: String,
    pub camera: String,
    pub microphone: String,
    pub screen: String,
    pub fullscreen_required: bool,
    pub focus_monitoring: bool,
    pub retention_days: i32,
}

const LEVELS: [&str; 3] = ["off", "optional", "on"];

#[allow(clippy::too_many_arguments)]
pub async fn create_policy(
    pool: &PgPool,
    ctx: &AuthContext,
    exam_type: &str,
    camera: &str,
    microphone: &str,
    screen: &str,
    fullscreen_required: bool,
    focus_monitoring: bool,
    retention_days: i32,
) -> Result<PolicyResponse, AppError> {
    require_permission(ctx, Resource::ProctoringPolicy, Action::Create)?;

    if !LEVELS.contains(&camera) {
        return Err(AppError::UnprocessableEntity("invalid_camera_level", "camera must be off, optional, or on".to_string()));
    }
    if !LEVELS.contains(&microphone) {
        return Err(AppError::UnprocessableEntity("invalid_microphone_level", "microphone must be off, optional, or on".to_string()));
    }
    if !LEVELS.contains(&screen) {
        return Err(AppError::UnprocessableEntity("invalid_screen_level", "screen must be off, optional, or on".to_string()));
    }
    if retention_days < 0 {
        return Err(AppError::UnprocessableEntity("invalid_retention_days", "retention_days must be a non-negative integer".to_string()));
    }

    let row = sqlx::query_as!(
        PolicyResponse,
        r#"insert into proctoring_policies (exam_type, camera, microphone, screen, fullscreen_required, focus_monitoring, retention_days)
           values ($1, $2, $3, $4, $5, $6, $7)
           returning id, exam_type, camera, microphone, screen, fullscreen_required, focus_monitoring, retention_days"#,
        exam_type,
        camera,
        microphone,
        screen,
        fullscreen_required,
        focus_monitoring,
        retention_days,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<PolicyResponse>, AppError> {
    let row = sqlx::query_as!(
        PolicyResponse,
        r#"select id, exam_type, camera, microphone, screen, fullscreen_required, focus_monitoring, retention_days from proctoring_policies where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// GET /proctoring-policies — any authenticated user, unfiltered, unpaginated.
pub async fn list_policies(pool: &PgPool) -> Result<Vec<PolicyResponse>, AppError> {
    let rows = sqlx::query_as!(PolicyResponse, r#"select id, exam_type, camera, microphone, screen, fullscreen_required, focus_monitoring, retention_days from proctoring_policies"#)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}
