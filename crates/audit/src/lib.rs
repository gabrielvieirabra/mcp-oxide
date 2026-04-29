//! Audit sinks.

#![deny(unsafe_code)]

use async_trait::async_trait;
use mcp_oxide_core::{audit::AuditRecord, providers::AuditSink, Result};

/// Emits records as a single JSON line at `info` level on the `audit` target.
#[cfg(feature = "stdout")]
#[derive(Debug, Default)]
pub struct StdoutAuditSink;

#[cfg(feature = "stdout")]
#[async_trait]
impl AuditSink for StdoutAuditSink {
    async fn emit(&self, record: &AuditRecord) -> Result<()> {
        let json = serde_json::to_string(record)
            .map_err(|e| mcp_oxide_core::Error::Internal(format!("audit serialize: {e}")))?;
        tracing::info!(target: "audit", "{}", json);
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "stdout"
    }
}
