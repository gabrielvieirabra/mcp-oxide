//! Provider traits — the pluggable core of mcp-oxide.
//!
//! Each trait has multiple implementations (first-party + community), selected
//! at startup from configuration. All traits are `async_trait` and
//! `Send + Sync` so they can live inside a shared `AppState`.

use crate::adapter::Adapter;
use crate::audit::AuditRecord;
use crate::error::{Error, Result};
use crate::identity::UserContext;
use crate::policy::{Decision, PolicyInput};
use crate::session::{BackendId, SessionId};
use crate::tool::Tool;
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[async_trait]
pub trait IdProvider: Send + Sync {
    /// Validate a bearer token and return the derived `UserContext`.
    async fn validate(&self, token: &str) -> Result<UserContext>;

    /// Force a refresh of signing keys (noop for providers without JWKS).
    async fn refresh_keys(&self) -> Result<()> {
        Ok(())
    }

    /// Short provider identifier (e.g. `oidc-generic`).
    fn kind(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Authorization
// ---------------------------------------------------------------------------

#[async_trait]
pub trait PolicyEngine: Send + Sync {
    async fn decide(&self, input: &PolicyInput<'_>) -> Result<Decision>;
    fn kind(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Deployment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DeploymentSpec {
    pub name: String,
    pub kind: DeploymentKind,
    pub adapter: Option<Adapter>,
    pub tool: Option<Tool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentKind {
    Adapter,
    Tool,
}

#[derive(Debug, Clone)]
pub struct DeploymentHandle {
    pub id: String,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeploymentStatus {
    pub ready: bool,
    pub replicas: u32,
    pub ready_replicas: u32,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub url: String,
    pub backend_id: BackendId,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub ts: String,
    pub line: String,
}

#[async_trait]
pub trait DeploymentProvider: Send + Sync {
    async fn apply(&self, spec: &DeploymentSpec) -> Result<DeploymentHandle>;
    async fn delete(&self, handle: &DeploymentHandle) -> Result<()>;
    async fn status(&self, handle: &DeploymentHandle) -> Result<DeploymentStatus>;
    async fn logs(&self, handle: &DeploymentHandle) -> Result<BoxStream<'static, LogLine>>;
    async fn endpoints(&self, handle: &DeploymentHandle) -> Result<Vec<Endpoint>>;
    fn kind(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Metadata store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub tenant: Option<String>,
    pub tags: Vec<String>,
}

#[async_trait]
pub trait MetadataStore: Send + Sync {
    async fn put_adapter(&self, a: &Adapter) -> Result<()>;
    async fn get_adapter(&self, name: &str) -> Result<Option<Adapter>>;
    async fn list_adapters(&self, filter: &Filter) -> Result<Vec<Adapter>>;
    async fn delete_adapter(&self, name: &str) -> Result<()>;

    async fn put_tool(&self, t: &Tool) -> Result<()>;
    async fn get_tool(&self, name: &str) -> Result<Option<Tool>>;
    async fn list_tools(&self, filter: &Filter) -> Result<Vec<Tool>>;
    async fn delete_tool(&self, name: &str) -> Result<()>;

    fn kind(&self) -> &'static str;
}

#[derive(Debug, Clone, Default)]
pub struct CreateAdapterRequest {
    pub name: String,
    pub description: Option<String>,
    pub image: String,
    pub endpoint_port: u16,
    pub endpoint_path: Option<String>,
    pub replicas: Option<u32>,
    pub env: Vec<EnvVarEntry>,
    pub secret_refs: Vec<SecretRefEntry>,
    pub required_roles: Vec<String>,
    pub tags: Vec<String>,
    pub resources: Option<ResourcesSpec>,
    pub health: Option<HealthProbeSpec>,
    pub session_affinity: Option<String>,
    pub labels: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct CreateToolRequest {
    pub name: String,
    pub description: Option<String>,
    pub image: String,
    pub endpoint_port: u16,
    pub endpoint_path: Option<String>,
    pub tool_definition: ToolDefinitionSpec,
    pub env: Vec<EnvVarEntry>,
    pub secret_refs: Vec<SecretRefEntry>,
    pub required_roles: Vec<String>,
    pub tags: Vec<String>,
    pub resources: Option<ResourcesSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateAdapterRequest {
    pub description: Option<String>,
    pub image: Option<String>,
    pub endpoint_port: Option<u16>,
    pub endpoint_path: Option<String>,
    pub replicas: Option<u32>,
    pub env: Option<Vec<EnvVarEntry>>,
    pub secret_refs: Option<Vec<SecretRefEntry>>,
    pub required_roles: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub resources: Option<ResourcesSpec>,
    pub health: Option<HealthProbeSpec>,
    pub session_affinity: Option<String>,
    pub labels: Option<std::collections::BTreeMap<String, String>>,
    pub revision: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateToolRequest {
    pub description: Option<String>,
    pub image: Option<String>,
    pub endpoint_port: Option<u16>,
    pub endpoint_path: Option<String>,
    pub tool_definition: Option<ToolDefinitionSpec>,
    pub env: Option<Vec<EnvVarEntry>>,
    pub secret_refs: Option<Vec<SecretRefEntry>>,
    pub required_roles: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub resources: Option<ResourcesSpec>,
    pub revision: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct EnvVarEntry {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct SecretRefEntry {
    pub name: String,
    pub provider: String,
    pub key: String,
}

#[derive(Debug, Clone, Default)]
pub struct ResourcesSpec {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct HealthProbeSpec {
    pub path: String,
    pub port: u16,
}

#[derive(Debug, Clone, Default)]
pub struct ToolDefinitionSpec {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    pub annotations: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Session store
// ---------------------------------------------------------------------------

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn resolve(&self, session_id: &SessionId, adapter: &str) -> Result<Option<BackendId>>;
    async fn bind(
        &self,
        session_id: &SessionId,
        adapter: &str,
        backend: BackendId,
        ttl: Duration,
    ) -> Result<()>;
    async fn drop_session(&self, session_id: &SessionId) -> Result<()>;
    fn kind(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SecretValue(pub Vec<u8>);

impl SecretValue {
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }
}

#[derive(Debug, Clone)]
pub struct SecretLookup {
    pub provider: String,
    pub key: String,
}

#[async_trait]
pub trait SecretProvider: Send + Sync {
    async fn get(&self, lookup: &SecretLookup) -> Result<SecretValue>;
    fn kind(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

#[async_trait]
pub trait AuditSink: Send + Sync {
    async fn emit(&self, record: &AuditRecord) -> Result<()>;
    fn kind(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Image registry
// ---------------------------------------------------------------------------

#[async_trait]
pub trait ImageRegistry: Send + Sync {
    /// Resolve a tag to an immutable digest reference.
    async fn resolve(&self, reference: &str) -> Result<String>;
    fn kind(&self) -> &'static str;
}

// Convenience: convert [`anyhow::Error`]-like strings into [`Error::Internal`].
pub fn internal<E: std::fmt::Display>(err: E) -> Error {
    Error::Internal(err.to_string())
}
