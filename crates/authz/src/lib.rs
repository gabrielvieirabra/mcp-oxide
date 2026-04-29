//! Authorization engines.

#![deny(unsafe_code)]

use async_trait::async_trait;
use mcp_oxide_core::{
    policy::{Decision, PolicyInput},
    providers::PolicyEngine,
    Result,
};

/// Denies everything. Default policy engine when nothing is configured.
#[derive(Debug, Default)]
pub struct DenyAllPolicyEngine;

#[async_trait]
impl PolicyEngine for DenyAllPolicyEngine {
    async fn decide(&self, _input: &PolicyInput<'_>) -> Result<Decision> {
        Ok(Decision::deny("default-deny"))
    }

    fn kind(&self) -> &'static str {
        "deny-all"
    }
}
