//! Secret providers.

#![deny(unsafe_code)]

use async_trait::async_trait;
use mcp_oxide_core::{
    providers::{SecretLookup, SecretProvider, SecretValue},
    Error, Result,
};

/// Reads secrets from process environment variables. `lookup.key` is the env
/// var name.
#[cfg(feature = "env")]
#[derive(Debug, Default)]
pub struct EnvSecretProvider;

#[cfg(feature = "env")]
#[async_trait]
impl SecretProvider for EnvSecretProvider {
    async fn get(&self, lookup: &SecretLookup) -> Result<SecretValue> {
        std::env::var(&lookup.key)
            .map(|s| SecretValue(s.into_bytes()))
            .map_err(|_| Error::NotFound(format!("env:{}", lookup.key)))
    }

    fn kind(&self) -> &'static str {
        "env"
    }
}
