use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::{auth as auth_service, token};
use crate::state::AppState;

// Port of auth-context.ts's resolveAuthContext, applied as an Axum
// `from_fn` function middleware (parelabs-backend's pattern, not a
// custom extractor) to every protected route's sub-router.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let ctx = resolve_auth_context(&state, req.headers()).await?;
    req.extensions_mut().insert(ctx);
    Ok(next.run(req).await)
}

pub async fn resolve_auth_context(state: &AppState, headers: &HeaderMap) -> Result<AuthContext, AppError> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let token_str = auth_header.strip_prefix("Bearer ").ok_or(AppError::Unauthorized)?;
    let claims = token::verify_access_token(token_str, &state.config.jwt_access_secret)?;
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| AppError::Unauthorized)?;

    // "Active" org — the caller's X-Organization-Id header if present
    // and a valid UUID, else the user's earliest-granted org. Null for
    // both if the user has no user_organization_roles row at all yet.
    let requested_org = headers
        .get("x-organization-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok());

    let role_row = match requested_org {
        Some(org_id) => auth_service::find_role_in_org(&state.db, user_id, org_id).await?,
        None => auth_service::find_default_role(&state.db, user_id).await?,
    };

    Ok(AuthContext {
        user_id,
        organization_id: role_row.as_ref().map(|r| r.organization_id),
        role: role_row.map(|r| r.role),
    })
}
