//! Shared application state assembled from configured providers.

use std::sync::Arc;

use mcp_oxide_audit::StdoutAuditSink;
use mcp_oxide_authz::DenyAllPolicyEngine;
use mcp_oxide_core::providers::{
    AuditSink, DeploymentProvider, IdProvider, MetadataStore, PolicyEngine, SecretProvider,
    SessionStore,
};
use mcp_oxide_deployment::NoopExternalProvider;
use mcp_oxide_identity::NoopIdProvider;
use mcp_oxide_metadata::InMemoryMetadataStore;
use mcp_oxide_secrets::EnvSecretProvider;
use mcp_oxide_session::InMemorySessionStore;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<dyn IdProvider>,
    pub authz: Arc<dyn PolicyEngine>,
    pub deployment: Arc<dyn DeploymentProvider>,
    pub metadata: Arc<dyn MetadataStore>,
    pub session: Arc<dyn SessionStore>,
    pub secrets: Arc<dyn SecretProvider>,
    pub audit: Arc<dyn AuditSink>,
    pub started_at: std::time::Instant,
}

impl AppState {
    #[allow(clippy::unused_async)]
    pub async fn bootstrap(_cfg: &Config) -> anyhow::Result<Self> {
        // Phase 0 wiring: only default/noop providers. Real wiring in Phase 1+.
        let s = Self {
            identity: Arc::new(NoopIdProvider),
            authz: Arc::new(DenyAllPolicyEngine),
            deployment: Arc::new(NoopExternalProvider),
            metadata: Arc::new(InMemoryMetadataStore::new()),
            session: Arc::new(InMemorySessionStore::new()),
            secrets: Arc::new(EnvSecretProvider),
            audit: Arc::new(StdoutAuditSink),
            started_at: std::time::Instant::now(),
        };
        tracing::info!(
            identity = s.identity.kind(),
            authz = s.authz.kind(),
            deployment = s.deployment.kind(),
            metadata = s.metadata.kind(),
            session = s.session.kind(),
            secrets = s.secrets.kind(),
            audit = s.audit.kind(),
            "providers wired"
        );
        Ok(s)
    }

    pub fn provider_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "identity":   self.identity.kind(),
            "authz":      self.authz.kind(),
            "deployment": self.deployment.kind(),
            "metadata":   self.metadata.kind(),
            "session":    self.session.kind(),
            "secrets":    self.secrets.kind(),
            "audit":      self.audit.kind(),
        })
    }
}
