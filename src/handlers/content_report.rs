use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::content_report::{self, CreateReportRequest, CreateReportResponse, TicketDetailResponse, TicketListQuery, TicketListResponse, UpdateTicketRequest};
use crate::state::AppState;

// POST /content-reports
pub async fn post_report(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Json(body): Json<CreateReportRequest>) -> Result<(StatusCode, Json<CreateReportResponse>), AppError> {
    Ok((StatusCode::CREATED, Json(content_report::create(&state.db, &ctx, body).await?)))
}

// GET /admin/content-tickets
pub async fn get_tickets(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<TicketListQuery>) -> Result<Json<TicketListResponse>, AppError> {
    Ok(Json(content_report::list(&state.db, &ctx, query).await?))
}

// GET /admin/content-tickets/{id}
pub async fn get_ticket(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<TicketDetailResponse>, AppError> {
    Ok(Json(content_report::detail(&state.db, &ctx, id).await?))
}

// PATCH /admin/content-tickets/{id}
pub async fn patch_ticket(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, Json(body): Json<UpdateTicketRequest>) -> Result<Json<TicketDetailResponse>, AppError> {
    Ok(Json(content_report::update(&state.db, &ctx, id, body).await?))
}

