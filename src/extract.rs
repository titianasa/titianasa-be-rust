use axum::extract::{FromRequest, Request};

use crate::errors::AppError;

// A request-body JSON extractor whose rejection goes through AppError's
// own {"error", "detail"} envelope, instead of axum::Json's default
// plain-text rejection body. Found via a live Bun-vs-Rust regression
// pass: axum's own `Json<T>` extractor rejection bypasses the app's
// error formatter entirely, on every route with a JSON body — a bad
// request currently gets a plain-text response no client's `ApiError`
// parser can handle. `axum::Json` itself is untouched and still used
// for every RESPONSE (`Json<T>` return values) — only handler function
// PARAMETERS that extract a request body are switched to this type.
pub struct ValidatedJson<T>(pub T);

impl<S, T> FromRequest<S> for ValidatedJson<T>
where
    axum::Json<T>: FromRequest<S, Rejection = axum::extract::rejection::JsonRejection>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ValidatedJson(value)),
            Err(rejection) => Err(AppError::UnprocessableEntity("invalid_json_body", rejection.body_text())),
        }
    }
}
