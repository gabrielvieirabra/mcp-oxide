//! mcp-oxide gateway binary entry point.

#![deny(unsafe_code)]

use std::process::ExitCode;

mod app;
mod config;
mod error;
mod routes;
mod state;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    mcp_oxide_observability::init_logging(cfg.logs.json);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %cfg.server.bind,
        "starting mcp-oxide gateway"
    );

    let state = state::AppState::bootstrap(&cfg).await?;
    let router = app::router(state);

    let listener = tokio::net::TcpListener::bind(cfg.server.bind).await?;
    tracing::info!(addr = %listener.local_addr()?, "listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("SIGINT received"),
        () = terminate => tracing::info!("SIGTERM received"),
    }
}
