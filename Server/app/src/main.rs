mod server;

use std::{net::SocketAddr, sync::Arc};

use anyhow::Context;
use tokio::net::TcpListener;
use tracing::info;

use crate::server::{AppState, build_router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bk_wiver_server=debug,tower_http=info".into()),
        )
        .with_target(false)
        .compact()
        .init();

    let bind_addr: SocketAddr = std::env::var("BK_WIVER_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
        .parse()
        .context("failed to parse BK_WIVER_SERVER_ADDR")?;

    let server_url =
        std::env::var("BK_WIVER_SERVER_URL").unwrap_or_else(|_| format!("http://{}", bind_addr));
    let database_url = std::env::var("BK_WIVER_DATABASE_URL")
        .context("BK_WIVER_DATABASE_URL must be set to a PostgreSQL connection string")?;

    let state = Arc::new(
        AppState::new(server_url, database_url)
            .await
            .context("failed to initialize application state")?,
    );
    let app = build_router(state);
    let listener = TcpListener::bind(bind_addr)
        .await
        .context("failed to bind tcp listener")?;

    info!("bk-wiver server listening on http://{}", bind_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server terminated with error")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};

        if let Ok(mut term_signal) = signal(SignalKind::terminate()) {
            let _ = term_signal.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
