//! `mock-mcp` — standalone mock MCP server driven by a YAML fixture file.
//!
//! Used by `deploy/smoke/docker-compose.yaml` to stand up a reproducible
//! multi-backend topology for hand-testing the gateway. The binary is also
//! useful outside of docker — point it at a fixture and curl it directly.

use clap::Parser;
use mcp_oxide_testing::{MockFixture, MockMcp};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "mock-mcp",
    about = "Mock MCP server driven by a YAML fixture file."
)]
struct Opts {
    /// Address to bind to, e.g. `0.0.0.0:8090`.
    #[arg(long, env = "MOCK_MCP_BIND", default_value = "0.0.0.0:8090")]
    bind: SocketAddr,

    /// Path to a YAML fixture file.
    #[arg(long, env = "MOCK_MCP_FIXTURE")]
    fixture: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let opts = Opts::parse();
    let fixture = match opts.fixture {
        Some(p) => MockFixture::from_yaml_path(&p)?,
        None => MockFixture::default(),
    };

    let mock = MockMcp::builder()
        .fixture(fixture)
        .bind(opts.bind)
        .build()
        .await?;

    tracing::info!(url = %mock.mcp_url(), "mock-mcp ready");

    // Hold until Ctrl-C / SIGTERM.
    tokio::select! {
        () = async { let _ = tokio::signal::ctrl_c().await; } => {
            tracing::info!("ctrl-c received, shutting down");
        }
        () = wait_for_sigterm() => {
            tracing::info!("SIGTERM received, shutting down");
        }
    }

    mock.shutdown().await;
    Ok(())
}

#[cfg(unix)]
async fn wait_for_sigterm() {
    use tokio::signal::unix::{signal, SignalKind};
    if let Ok(mut s) = signal(SignalKind::terminate()) {
        let _ = s.recv().await;
    }
}

#[cfg(not(unix))]
async fn wait_for_sigterm() {
    std::future::pending::<()>().await;
}
