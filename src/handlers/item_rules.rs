use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::module::{ProctorConfigRequest, SetApprovalRequest, UpdateItemGuardsRequest};
use crate::services::item_proctor::{self, EventRequest, ItemProctorConfigResponse, SessionEvent, SessionState, SessionSummary, StartSessionRequest};
use crate::services::item_progress::{self, ProgressRow};
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::{item_guard, module_item, org_class, org_class_session};
use crate::state::AppState;

// The module builder's per-item rules: access gates, attendance guards,
// proctoring — and the learner-side calls those rules depend on.

// PATCH /module-items/{id}/guards — "Aturan Akses & Guard" + "Attendance
// Guard". Settings, not content: editable after publish.
pub async fn patch_guards(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(item_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<UpdateItemGuardsRequest>,
) -> Result<Json<Value>, AppError> {
    require_permission(&ctx, Resource::ModuleItem, Action::Create)?;
    let guard = item_guard::validate_guard(body.guard_config.as_ref())?;
    let attendance = item_guard::validate_attendance_guard(body.attendance_guard.as_ref())?;
    let guard_json = guard.map(|g| serde_json::to_value(g).unwrap_or(Value::Null));
    let attendance_json = attendance.map(|g| serde_json::to_value(g).unwrap_or(Value::Null));
    let updated = sqlx::query!(
        r#"update module_items set guard_config = $2, attendance_guard = $3, updated_at = now() where id = $1 and node_type = 'item'"#,
        item_id,
        guard_json,
        attendance_json,
    )
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound("module_item_not_found"));
    }
    Ok(Json(json!({ "guard_config": guard_json, "attendance_guard": attendance_json })))
}

// POST /module-items/{id}/complete — the learner finished reading an
// article. (A quiz completes itself on submit.)
pub async fn post_complete(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(item_id): Path<Uuid>) -> Result<StatusCode, AppError> {
    let item = module_item::get_detail(&state.db, &ctx, item_id).await?;
    if item.content_type.as_deref() != Some("article") {
        return Err(AppError::UnprocessableEntity("not_an_article", "kuis selesai saat dikumpulkan, bukan lewat tombol ini".to_string()));
    }
    item_progress::record_completion(&state.db, ctx.user_id, item_id, None, "self_learning").await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /module-items/{id}/progress
pub async fn get_progress(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(item_id): Path<Uuid>) -> Result<Json<Vec<ProgressRow>>, AppError> {
    Ok(Json(item_progress::list_for_item(&state.db, &ctx, item_id).await?))
}

// POST /module-items/{id}/progress/{user_id}/approve
pub async fn post_approval(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((item_id, user_id)): Path<(Uuid, Uuid)>,
    ValidatedJson(body): ValidatedJson<SetApprovalRequest>,
) -> Result<StatusCode, AppError> {
    item_progress::set_approval(&state.db, &ctx, item_id, user_id, body.approved).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /module-items/{id}/proctor-config
pub async fn get_proctor_config(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(item_id): Path<Uuid>) -> Result<Json<ItemProctorConfigResponse>, AppError> {
    Ok(Json(item_proctor::get_item_config(&state.db, &ctx, item_id).await?))
}

// PUT /module-items/{id}/proctor-config
pub async fn put_proctor_config(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(item_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<ProctorConfigRequest>,
) -> Result<Json<ItemProctorConfigResponse>, AppError> {
    Ok(Json(item_proctor::set_item_config(&state.db, &ctx, item_id, body.config).await?))
}

// GET /module-items/{id}/effective-proctor-config — for the exam page;
// open to anyone who may open the item.
pub async fn get_effective_proctor_config(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(item_id): Path<Uuid>) -> Result<Json<Value>, AppError> {
    module_item::get_detail(&state.db, &ctx, item_id).await?;
    Ok(Json(json!({ "effective": item_proctor::effective_for_item(&state.db, item_id).await? })))
}

// GET /modules/{id}/proctor-config
pub async fn get_module_proctor_config(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(module_id): Path<Uuid>) -> Result<Json<Value>, AppError> {
    Ok(Json(item_proctor::get_module_config(&state.db, &ctx, module_id).await?))
}

// PUT /modules/{id}/proctor-config
pub async fn put_module_proctor_config(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(module_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<ProctorConfigRequest>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(item_proctor::set_module_config(&state.db, &ctx, module_id, body.config).await?))
}

// POST /module-items/{id}/proctor-sessions
pub async fn post_proctor_session(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(item_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<StartSessionRequest>,
) -> Result<Json<SessionState>, AppError> {
    Ok(Json(item_proctor::start_session(&state.db, &ctx, item_id, body).await?))
}

// GET /module-items/{id}/proctor-sessions
pub async fn get_proctor_sessions(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(item_id): Path<Uuid>) -> Result<Json<Vec<SessionSummary>>, AppError> {
    Ok(Json(item_proctor::list_sessions(&state.db, &ctx, item_id).await?))
}

// POST /quiz-proctor-sessions/{id}/events
pub async fn post_proctor_event(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<EventRequest>,
) -> Result<Json<SessionState>, AppError> {
    Ok(Json(item_proctor::record_event(&state.db, &ctx, session_id, body).await?))
}

// GET /quiz-proctor-sessions/{id}/events
pub async fn get_proctor_events(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(session_id): Path<Uuid>) -> Result<Json<Vec<SessionEvent>>, AppError> {
    Ok(Json(item_proctor::session_events(&state.db, &state.config, state.storage.as_ref(), &ctx, session_id).await?))
}

// POST /quiz-proctor-sessions/{id}/unlock
pub async fn post_proctor_unlock(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(session_id): Path<Uuid>) -> Result<StatusCode, AppError> {
    item_proctor::unlock(&state.db, &ctx, session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /org-class-sessions/{id}/gate-status — whether the calling learner
// may check in, and if not, what to finish first.
pub async fn get_gate_status(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(session_id): Path<Uuid>) -> Result<Json<Value>, AppError> {
    let session = org_class_session::find_by_id(&state.db, session_id).await?.ok_or(AppError::NotFound("class_session_not_found"))?;
    org_class::assert_is_member_or_manager(&state.db, &ctx, session.class_id).await?;
    let blockers = item_guard::attendance_blockers(&state.db, ctx.user_id, session.class_id).await?;
    Ok(Json(json!({ "open": blockers.is_empty(), "blockers": blockers })))
}
