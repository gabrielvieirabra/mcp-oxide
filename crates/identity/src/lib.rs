//! Identity providers.

#![deny(unsafe_code)]

use async_trait::async_trait;
use mcp_oxide_core::{identity::UserContext, providers::IdProvider, Error, Result};

pub mod claims;

#[cfg(feature = "static-jwt")]
pub mod static_jwt;
#[cfg(feature = "static-jwt")]
pub use static_jwt::{StaticJwtConfig, StaticJwtProvider};

#[cfg(feature = "oidc-generic")]
pub mod oidc;
#[cfg(feature = "oidc-generic")]
pub use oidc::{OidcConfig, OidcProvider};

/// Rejects every token. Default when nothing is configured.
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
