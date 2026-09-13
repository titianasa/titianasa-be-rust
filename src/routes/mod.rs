pub mod learning_profile;
pub mod user_data_consent;
pub mod ai;
pub mod assessment;
pub mod attendance;
pub mod auth;
pub mod drive;
pub mod economy;
pub mod gamification;
pub mod learning;
pub mod marketplace;
pub mod messaging;
pub mod module;
pub mod org_class;
pub mod organization;
pub mod proctoring;
pub mod program;
pub mod question;
pub mod speaking_room;

use axum::{
    http::Method,
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowHeaders, AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

use crate::handlers;
use crate::middleware::auth::auth_middleware;
use crate::state::AppState;

// Mirrors parelabs-backend's routes/mod.rs: one middleware stack applied
// once at the root, per-feature sub-routers merged in. CORS settings
// (origin/methods/headers) port index.ts's `cors({...})` call exactly —
// titian-web is the only client, so origin is one fixed value, not a
// wildcard-subdomain predicate like parelabs'.
//
// All protected feature routers are merged into ONE subtree first, then
// auth_middleware is layered on that single merge — not once per feature
// module, which would run the same auth check redundantly per layer.
pub fn create_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(state.config.frontend_origin.parse().unwrap()))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
        .allow_headers(AllowHeaders::list([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            "x-organization-id".parse().unwrap(),
        ]));

    let protected = Router::new()
        .route("/events", post(handlers::client_events::post_events))
        .merge(ai::protected_routes())
        .merge(auth::protected_routes())
        .merge(organization::protected_routes())
        .merge(module::protected_routes())
        .merge(org_class::protected_routes())
        .merge(program::protected_routes())
        .merge(question::protected_routes())
        .merge(assessment::protected_routes())
        .merge(learning::protected_routes())
        .merge(gamification::protected_routes())
        .merge(economy::protected_routes())
        .merge(learning_profile::protected_routes())
        .merge(marketplace::protected_routes())
        .merge(attendance::protected_routes())
        .merge(speaking_room::protected_routes())
        .merge(drive::protected_routes())
        .merge(messaging::protected_routes())
        .merge(proctoring::protected_routes())
        .merge(user_data_consent::protected_routes())
        .layer(from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .route("/health", get(handlers::health::get_health))
        .merge(auth::public_routes())
        .merge(marketplace::public_routes())
        .merge(messaging::public_routes())
        .merge(module::public_routes())
        .merge(protected)
        // A live Bun-vs-Rust regression pass found axum's own default
        // 404 (an empty body, no content-type) on any unmatched
        // path/method — breaking the app's {"error","detail"} contract
        // just like the unhandled-JSON-rejection gap `extract.rs` fixes.
        // Bun's own behavior here (any Elysia-internal "route not
        // found" falls into the generic onError catch-all, emitting a
        // MISLEADING 500 internal_error) isn't a deliberate business
        // rule worth reproducing bug-for-bug — a genuinely unmatched
        // route has no real client-facing meaning to preserve, unlike
        // the documented quirks elsewhere in this port.
        .fallback(crate::handlers::health::not_found)
        .layer(cors)
        // Curriculum responses are large, deeply repetitive JSON — the
        // learning-path browser alone is ~440 KB uncompressed. Most of
        // this app's users are on metered mobile data, so compress
        // everything rather than special-casing one endpoint.
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
