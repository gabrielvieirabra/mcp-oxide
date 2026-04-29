//! Identity providers.
//!
//! Phase 0 exposes a single `NoopIdProvider` used by the gateway binary until
//! Phase 1 lands the `oidc-generic` implementation.

#![deny(unsafe_code)]

use async_trait::async_trait;
use mcp_oxide_core::{identity::UserContext, providers::IdProvider, Error, Result};

/// Rejects every token. Default provider when nothing is configured.
#[derive(Debug, Default)]
pub struct NoopIdProvider;

#[async_trait]
impl IdProvider for NoopIdProvider {
    async fn validate(&self, _token: &str) -> Result<UserContext> {
        Err(Error::Unauthenticated(
            "no identity provider configured".into(),
        ))
    }

    fn kind(&self) -> &'static str {
        "noop"
    }
}
