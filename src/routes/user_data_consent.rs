use axum::{routing::get, Router};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/me/consents",
        get(handlers::user_data_consent::get_my_consents).post(handlers::user_data_consent::post_my_consent),
    )
}
