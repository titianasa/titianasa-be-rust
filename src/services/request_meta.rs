use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};

// Admin Pusat "Peserta" — the one place a request's IP + a human device
// label get pulled out, shared by the presence touch (middleware/auth.rs,
// every authenticated request) and login capture (services/auth.rs,
// issue_tokens). Deliberately NOT a header-based IP lookup
// (X-Forwarded-For etc.) — this repo has no reverse proxy configured
// anywhere, so a header would just be the client handing us whatever IP
// it wants. `ConnectInfo` is the actual TCP peer, wired in main.rs via
// `into_make_service_with_connect_info`. Revisit this the day a proxy
// sits in front of the app.
pub struct RequestMeta {
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub device_label: String,
}

/// `Option<ConnectInfo<T>>` is NOT a valid axum extractor on its own —
/// axum only special-cases `Option<T>` for types implementing its own
/// `OptionalFromRequestParts`, which `ConnectInfo` doesn't (confirmed
/// the hard way: requiring bare `ConnectInfo<SocketAddr>` 500s every
/// integration test, since those dispatch requests via `Router::oneshot`
/// rather than a real listener — `into_make_service_with_connect_info`
/// is what inserts the extension, and `oneshot` skips that entirely).
/// This wrapper reads the extension by hand and is infallible, so it
/// works identically whether or not a real connection produced it.
pub struct MaybeConnectInfo(pub Option<SocketAddr>);

impl<S> FromRequestParts<S> for MaybeConnectInfo
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(MaybeConnectInfo(parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|ci| ci.0)))
    }
}

pub fn extract(headers: &HeaderMap, connect_info: Option<&ConnectInfo<SocketAddr>>) -> RequestMeta {
    let ip = connect_info.map(|ConnectInfo(addr)| addr.ip());
    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok()).map(str::to_string);
    let device_label = user_agent.as_deref().map(device_label_for).unwrap_or_else(|| "Tidak diketahui".to_string());
    RequestMeta { ip, user_agent, device_label }
}

fn device_label_for(user_agent: &str) -> String {
    let parser = woothee::parser::Parser::new();
    match parser.parse(user_agent) {
        Some(result) if result.os != "UNKNOWN" && result.name != "UNKNOWN" => format!("{} · {}", result.os, result.name),
        Some(result) if result.name != "UNKNOWN" => result.name.to_string(),
        _ => "Tidak diketahui".to_string(),
    }
}
