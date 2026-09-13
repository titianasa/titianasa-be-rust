use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::{json, Value};
use uuid::Uuid;

// Port of titian-backend-bun/src/error.ts's AppError. The JSON body shape
// is the existing wire contract titian-web's ApiError class already
// parses — `{"error": "snake_case_code", "detail": ...}` — NOT
// parelabs-backend's `{"error": {"code", "message"}}` convention. This is
// the one place this project deliberately diverges from parelabs' error
// shape while still copying its mechanics (thiserror enum + IntoResponse).
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("invalid_token")]
    InvalidToken,
    #[error("invalid_refresh_token")]
    InvalidRefreshToken,
    #[error("forbidden")]
    Forbidden,
    // Port of error.ts's forbiddenWithCode — a 403 with a specific code
    // instead of the generic "forbidden" (e.g. lesson_not_published).
    #[error("forbidden")]
    ForbiddenWithCode(&'static str),
    #[error("not_found")]
    NotFound(&'static str),
    // Port of error.ts's conflict(code) — 409, a generic "this already
    // happened, can't happen again" (e.g. order_already_exists,
    // certificate_already_issued, already_reviewed).
    #[error("conflict")]
    Conflict(&'static str),
    #[error("unprocessable_entity")]
    UnprocessableEntity(&'static str, String),
    // Port of error.ts's attemptAlreadyInProgress — 409, a rare exception
    // to the standard {error, detail} envelope: carries `attempt_id` so
    // the client can resume the existing attempt instead of retrying
    // blindly.
    #[error("conflict")]
    AttemptAlreadyInProgress(Uuid),
    // Port of error.ts's missingRequiredAnswers — 422, carries the list
    // of question ids the submit was missing an answer for.
    #[error("unprocessable_entity")]
    MissingRequiredAnswers(Vec<Uuid>),
    // Port of error.ts's insufficientCredit — 402, not enough credit
    // balance to cover an AI task's charge (checked before the provider
    // is called, ADR-0005).
    #[error("payment_required")]
    InsufficientCredit { required: i64, balance: i64 },
    // Port of google_meet_provider.ts's inline `new AppError(502, ...)` —
    // the only 502 anywhere in the codebase, raised when the Meet OAuth
    // token exchange or a Meet API call itself fails (not a generic
    // internal error: the caller/UI should be able to tell "our bug" from
    // "Google's API rejected this").
    #[error("bad_gateway")]
    BadGateway(&'static str, String),
    // Port of asset_handler.ts's AppError.payloadTooLarge — 413, raised
    // when an uploaded file exceeds config.asset_max_bytes.
    #[error("payload_too_large")]
    PayloadTooLarge(&'static str),
    // P39-005 — 429, `client_events::check_rate_limit` when one user's
    // `POST /events` batches exceed the per-minute ceiling. The first
    // (and so far only) 429 in this codebase.
    #[error("too_many_requests")]
    TooManyRequests(&'static str),
    // Port of error.ts's aiOutputValidationFailed — 422. Originally a
    // dedicated no-detail factory; now carries the real underlying
    // reason (the provider's own error, or why the output couldn't be
    // used) so a failure reads as something other than a bare error
    // code — the retry loop already tried again on the SAME provider
    // before giving up (never falls back to a different provider), so
    // this is the one message the caller actually gets to see.
    #[error("unprocessable_entity")]
    AiOutputValidationFailed(Option<String>),
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("internal error")]
    Internal(#[from] anyhow::Error),
}

// storage.rs's StorageError has no AppError-visible code of its own in
// the Bun original either — it propagates uncaught to the global error
// handler, which wraps it as a generic 500 internal_error.
impl From<crate::services::storage::StorageError> for AppError {
    fn from(e: crate::services::storage::StorageError) -> Self {
        AppError::Internal(anyhow::anyhow!(e))
    }
}

// Postgres SQLSTATE 23505 = unique_violation. Returns the violated
// constraint's name so the response says WHICH uniqueness rule broke.
fn unique_violation_constraint(e: &sqlx::Error) -> Option<String> {
    let db_err = e.as_database_error()?;
    if db_err.code().as_deref() != Some("23505") {
        return None;
    }
    Some(db_err.constraint().unwrap_or("unique").to_string())
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, body): (StatusCode, Value) = match &self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, json!({"error": "unauthorized", "detail": null})),
            AppError::InvalidToken => (StatusCode::UNAUTHORIZED, json!({"error": "invalid_token", "detail": null})),
            AppError::InvalidRefreshToken => {
                (StatusCode::UNAUTHORIZED, json!({"error": "invalid_refresh_token", "detail": null}))
            }
            AppError::Forbidden => (StatusCode::FORBIDDEN, json!({"error": "forbidden", "detail": null})),
            AppError::ForbiddenWithCode(code) => (StatusCode::FORBIDDEN, json!({"error": code, "detail": null})),
            AppError::NotFound(code) => (StatusCode::NOT_FOUND, json!({"error": code, "detail": null})),
            AppError::Conflict(code) => (StatusCode::CONFLICT, json!({"error": code, "detail": null})),
            AppError::AttemptAlreadyInProgress(attempt_id) => {
                (StatusCode::CONFLICT, json!({"error": "attempt_already_in_progress", "attempt_id": attempt_id}))
            }
            AppError::MissingRequiredAnswers(missing) => {
                (StatusCode::UNPROCESSABLE_ENTITY, json!({"error": "missing_required_answers", "missing": missing}))
            }
            AppError::InsufficientCredit { required, balance } => {
                (StatusCode::PAYMENT_REQUIRED, json!({"error": "insufficient_credit", "required": required, "balance": balance}))
            }
            AppError::UnprocessableEntity(code, detail) => {
                (StatusCode::UNPROCESSABLE_ENTITY, json!({"error": code, "detail": detail}))
            }
            AppError::BadGateway(code, detail) => (StatusCode::BAD_GATEWAY, json!({"error": code, "detail": detail})),
            AppError::PayloadTooLarge(code) => (StatusCode::PAYLOAD_TOO_LARGE, json!({"error": code, "detail": null})),
            AppError::TooManyRequests(code) => (StatusCode::TOO_MANY_REQUESTS, json!({"error": code, "detail": null})),
            AppError::AiOutputValidationFailed(detail) => (StatusCode::UNPROCESSABLE_ENTITY, json!({"error": "ai_output_validation_failed", "detail": detail})),
            // A unique-key clash is the CALLER re-sending a value that
            // already exists (e.g. modules.code), not a server fault —
            // it used to surface as an opaque 500 internal_error, which
            // gave the client nothing to act on. Everything else from
            // the database really is a 500.
            AppError::Database(e) => match unique_violation_constraint(e) {
                Some(constraint) => {
                    tracing::warn!(%constraint, "unique constraint violation");
                    (
                        StatusCode::CONFLICT,
                        json!({"error": "duplicate_value", "detail": format!("value already exists (constraint: {constraint})")}),
                    )
                }
                None => {
                    tracing::error!(error = ?e, "database error");
                    (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "internal_error", "detail": null}))
                }
            },
            AppError::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "internal_error", "detail": null}))
            }
        };
        (status, Json(body)).into_response()
    }
}
