use std::sync::Arc;

use axum::{extract::State, Extension, Json};

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct SubjectResponse {
    pub id: uuid::Uuid,
    pub code: String,
    pub name: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ListSubjectsResponse {
    pub items: Vec<SubjectResponse>,
}

// GET /subjects — every authoring form in Content Studio (new module, new
// program, new question bank, AI-generate) needs this to offer a real
// subject picker instead of one hardcoded default.
pub async fn get_subjects(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
) -> Result<Json<ListSubjectsResponse>, AppError> {
    let rows = crate::services::subject::list(&state.db).await?;
    Ok(Json(ListSubjectsResponse {
        items: rows.into_iter().map(|r| SubjectResponse { id: r.id, code: r.code, name: r.name }).collect(),
    }))
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateSubjectRequest {
    pub name: String,
}

// POST /subjects — the answer to "what if the subject I need isn't in
// the list": same permission gate as creating a module, since a new
// subject only really matters as something to attach a module to.
pub async fn post_subject(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateSubjectRequest>,
) -> Result<(axum::http::StatusCode, Json<SubjectResponse>), AppError> {
    require_permission(&ctx, Resource::Module, Action::Create)?;
    let row = crate::services::subject::create(&state.db, &body.name).await?;
    Ok((axum::http::StatusCode::CREATED, Json(SubjectResponse { id: row.id, code: row.code, name: row.name })))
}
