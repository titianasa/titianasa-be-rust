use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::certificate::{self, CertificateResponse, VerifyResponse};
use crate::state::AppState;

// POST /cohorts/{id}/enrollments/{enrollment_id}/certificate
pub async fn post_certificate(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((cohort_id, enrollment_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<CertificateResponse>), AppError> {
    let result = certificate::issue_certificate(&state.db, &state.config, &ctx, cohort_id, enrollment_id).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /certificates/{code}/verify — PUBLIC, no auth extractor.
pub async fn get_verify(State(state): State<Arc<AppState>>, Path(code): Path<String>) -> Result<Json<VerifyResponse>, AppError> {
    Ok(Json(certificate::verify(&state.db, &code).await?))
}

#[derive(serde::Serialize)]
#[serde(untagged)]
pub enum CertificateForEnrollmentResponse {
    Issued { issued: bool, #[serde(flatten)] certificate: CertificateResponse },
    NotIssued { issued: bool },
}

// GET /enrollments/{id}/certificate
pub async fn get_certificate_for_enrollment(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(enrollment_id): Path<Uuid>,
) -> Result<Json<CertificateForEnrollmentResponse>, AppError> {
    let result = certificate::get_certificate_for_enrollment(&state.db, &ctx, enrollment_id).await?;
    Ok(Json(match result {
        Some(certificate) => CertificateForEnrollmentResponse::Issued { issued: true, certificate },
        None => CertificateForEnrollmentResponse::NotIssued { issued: false },
    }))
}
