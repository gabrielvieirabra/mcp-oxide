//! Observability wiring: logs, metrics, tracing.
//!
//! Phase 0 ships the log subscriber init + a Prometheus recorder handle.

#![deny(unsafe_code)]

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialise the global log/tracing subscriber.
///
/// * `json = true` → structured JSON (production).
/// * `json = false` → human-readable pretty format (dev).
pub fn init_logging(json: bool) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,hyper=warn"));

    let registry = tracing_subscriber::registry().with(filter);

    if json {
        registry
            .with(
                fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_target(true),
            )
            .init();
    } else {
        registry.with(fmt::layer().with_target(true)).init();
    }
}

#[cfg(feature = "prometheus")]
pub mod prom {
    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

    pub fn install() -> Result<PrometheusHandle, String> {
        PrometheusBuilder::new()
            .install_recorder()
            .map_err(|e| format!("prometheus install: {e}"))
    }
}
