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
