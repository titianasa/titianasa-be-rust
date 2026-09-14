use axum::extract::{Path, Query};
use axum::{extract::State, Extension, Json};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::admin::{AuditLogQuery, MetricsPeriodQuery, ParticipantSearchQuery};
use crate::models::responses::admin::AuditLogListResponse;
use crate::services::admin_audit;
use crate::services::admin_metrics::{self, KurikulumResponse, OperasionalAiResponse, OrganisasiResponse, PenjualanResponse, RingkasanResponse};
use crate::services::admin_participants::{self, ParticipantDetailResponse, ParticipantSummary, PesertaResponse};
use crate::services::ai_settings::{self, CatalogEntry, PatchCatalogInput, RoleSettingRow, RoleTestResult, SaveRoleInput};
use crate::services::permissions::{require_permission, Action, Resource};
use crate::state::AppState;

// GET /admin/audit-log
pub async fn get_audit_log(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Query(query): Query<AuditLogQuery>,
) -> Result<Json<AuditLogListResponse>, AppError> {
    Ok(Json(admin_audit::list(&state.db, &ctx, query.cursor, query.limit).await?))
}

// GET /admin/ai/catalog — read-only, gated View (Manage is enforced
// inside ai_settings::patch_catalog_entry for the write below).
pub async fn get_ai_catalog(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Vec<CatalogEntry>>, AppError> {
    require_permission(&ctx, Resource::AdminPusat, Action::View)?;
    Ok(Json(ai_settings::catalog_list(&state.db).await?))
}

// PATCH /admin/ai/catalog/{id}
pub async fn patch_ai_catalog(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<PatchCatalogInput>,
) -> Result<Json<CatalogEntry>, AppError> {
    Ok(Json(ai_settings::patch_catalog_entry(&state.db, &ctx, id, body).await?))
}

// POST /admin/ai/catalog
pub async fn post_ai_catalog(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Json(body): Json<ai_settings::CreateCatalogInput>) -> Result<(axum::http::StatusCode, Json<CatalogEntry>), AppError> {
    Ok((axum::http::StatusCode::CREATED, Json(ai_settings::create_catalog_entry(&state.db, &ctx, body).await?)))
}

// GET /admin/ai/roles
pub async fn get_ai_roles(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Vec<RoleSettingRow>>, AppError> {
    require_permission(&ctx, Resource::AdminPusat, Action::View)?;
    Ok(Json(ai_settings::role_list(&state.db).await?))
}

// PUT /admin/ai/roles/{role}
pub async fn put_ai_role(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(role): Path<String>,
    ValidatedJson(body): ValidatedJson<SaveRoleInput>,
) -> Result<Json<RoleSettingRow>, AppError> {
    Ok(Json(ai_settings::save_role(&state.db, &ctx, &role, body).await?))
}

// POST /admin/ai/roles/{role}/test
pub async fn post_ai_role_test(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(role): Path<String>) -> Result<Json<RoleTestResult>, AppError> {
    Ok(Json(ai_settings::test_role(&state.db, &state.config, &ctx, state.text_ai_provider.as_ref(), &role).await?))
}

// GET /admin/metrics/ringkasan
pub async fn get_metrics_ringkasan(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<RingkasanResponse>, AppError> {
    Ok(Json(admin_metrics::ringkasan(&state.db, &ctx).await?))
}

// GET /admin/metrics/penjualan?from&to
pub async fn get_metrics_penjualan(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<MetricsPeriodQuery>) -> Result<Json<PenjualanResponse>, AppError> {
    let (default_from, default_to) = admin_metrics::default_period();
    Ok(Json(admin_metrics::penjualan(&state.db, &ctx, query.from.unwrap_or(default_from), query.to.unwrap_or(default_to)).await?))
}

// GET /admin/metrics/kurikulum
pub async fn get_metrics_kurikulum(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<KurikulumResponse>, AppError> {
    Ok(Json(admin_metrics::kurikulum(&state.db, &ctx).await?))
}

// GET /admin/metrics/operasional-ai?from&to
pub async fn get_metrics_operasional_ai(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<MetricsPeriodQuery>) -> Result<Json<OperasionalAiResponse>, AppError> {
    let (default_from, default_to) = admin_metrics::default_period();
    Ok(Json(admin_metrics::operasional_ai(&state.db, &ctx, query.from.unwrap_or(default_from), query.to.unwrap_or(default_to)).await?))
}

// GET /admin/metrics/organisasi
pub async fn get_metrics_organisasi(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<OrganisasiResponse>, AppError> {
    Ok(Json(admin_metrics::organisasi(&state.db, &ctx).await?))
}

// GET /admin/metrics/peserta?from&to
pub async fn get_metrics_peserta(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<MetricsPeriodQuery>) -> Result<Json<PesertaResponse>, AppError> {
    let (default_from, default_to) = admin_metrics::default_period();
    Ok(Json(admin_participants::peserta(&state.db, &ctx, query.from.unwrap_or(default_from), query.to.unwrap_or(default_to)).await?))
}

// GET /admin/participants/search?q=&limit=
pub async fn search_participants(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Query(query): Query<ParticipantSearchQuery>) -> Result<Json<Vec<ParticipantSummary>>, AppError> {
    Ok(Json(admin_participants::search_participants(&state.db, &ctx, &query.q, query.limit.unwrap_or(20)).await?))
}

// GET /admin/participants/{user_id}
pub async fn get_participant_detail(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(user_id): Path<Uuid>) -> Result<Json<ParticipantDetailResponse>, AppError> {
    Ok(Json(admin_participants::participant_detail(&state.db, &ctx, user_id).await?))
}
