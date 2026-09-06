use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// Port of parelabs-backend's observability/logging.rs, minus the
// production JSON-vs-pretty branch and metrics (no Prometheus wired up
// yet — nothing in titian-backend-bun exposes metrics today either, so
// there's no contract to match; add it in a later phase if needed).
pub fn setup() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,titian_backend_rust=debug"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
