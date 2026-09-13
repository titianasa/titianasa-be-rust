use std::sync::Arc;
use tokio::net::TcpListener;

use titian_backend_rust::{db, observability, routes, state::AppState, Config};

// Mirrors parelabs-backend's main.rs 6-step bootstrap sequence exactly.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;

    observability::setup();

    let state = Arc::new(AppState::init(config).await?);

    db::run_migrations(&state.db).await?;

    // P39-003 — a standing safety net so `learning_events` always has a
    // partition for "now" and "next month" to insert into, even past
    // whatever range the last migration pre-created. A proper monthly
    // job belongs in Phase 40's job queue; this is what exists before
    // that queue does.
    titian_backend_rust::services::learning_event::ensure_current_partitions(&state.db).await?;
    // P39-007 — drops any raw-event partition past its 24-month
    // retention window. Same "boot-time safety net until Phase 40 has a
    // real scheduler" reasoning as the line above.
    let dropped_partitions = titian_backend_rust::services::learning_event::enforce_retention(&state.db).await?;
    if !dropped_partitions.is_empty() {
        tracing::info!(?dropped_partitions, "learning_events retention: dropped partitions past 24 months");
    }

    let app = routes::create_router(state.clone());

    let listener = TcpListener::bind(&state.config.bind_addr).await?;
    tracing::info!("titian-backend-rust listening on {}", state.config.bind_addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
