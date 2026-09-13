// P39-007 (ADR-0013 "Privasi") — the consent surface a learner (or,
// for SD/SMP jenjang, a guardian on their behalf) uses. `learning_event
// ::record`'s own gate is what actually enforces this; these two
// endpoints are how a consent gets set in the first place.

use axum::{extract::State, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::services::user_data_consent;
use crate::state::AppState;

// GET /me/consents — every known kind, defaulting an unset one to
// `granted: false` (see user_data_consent::list_for_user).
pub async fn get_my_consents(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<Vec<user_data_consent::ConsentState>>, AppError> {
    Ok(Json(user_data_consent::list_for_user(&state.db, ctx.user_id).await?))
}

#[derive(Debug, serde::Deserialize)]
pub struct SetConsentRequest {
    pub kind: String,
    pub granted: bool,
    /// Only meaningful (and required to be `true`) when granting for an
    /// SD/SMP-jenjang learner — see `user_data_consent::set_consent`.
    #[serde(default)]
    pub guardian_confirmed: bool,
}

// POST /me/consents
pub async fn post_my_consent(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<SetConsentRequest>) -> Result<Json<user_data_consent::ConsentState>, AppError> {
    Ok(Json(user_data_consent::set_consent(&state.db, ctx.user_id, &body.kind, body.granted, ctx.user_id, body.guardian_confirmed, state.config.consent_guardian_confirmation_required).await?))
}
