//! Shared application state assembled from configured providers.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::Algorithm;
use mcp_oxide_audit::StdoutAuditSink;
use mcp_oxide_authz::{DenyAllPolicyEngine, YamlRbacEngine};
use mcp_oxide_core::providers::{
    AuditSink, DeploymentProvider, IdProvider, MetadataStore, PolicyEngine, SecretProvider,
    SessionStore,
};
use mcp_oxide_deployment::NoopExternalProvider;
#[cfg(feature = "docker")]
use mcp_oxide_deployment::DockerProvider;
use mcp_oxide_identity::{
    claims::ClaimExtractor, NoopIdProvider, OidcConfig, OidcProvider, StaticJwtConfig,
    StaticJwtProvider,
};
use mcp_oxide_metadata::{InMemoryMetadataStore, SqliteMetadataStore};
use mcp_oxide_secrets::EnvSecretProvider;
use mcp_oxide_session::InMemorySessionStore;

use crate::config::{AuthzConfig, Config, DeploymentConfig, IdentityConfig, MetadataStoreConfig, StaticAdapter};

/// Static adapter entry resolved to an upstream URL + policy metadata.
#[derive(Debug, Clone)]
pub struct ResolvedAdapter {
    pub name: String,
    pub upstream: String,
    pub required_roles: Vec<String>,
    pub tags: Vec<String>,
}

impl From<StaticAdapter> for ResolvedAdapter {
    fn from(v: StaticAdapter) -> Self {
        Self {
            name: v.name,
            upstream: v.upstream,
            required_roles: v.required_roles,
            tags: v.tags,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<dyn IdProvider>,
    pub authz: Arc<dyn PolicyEngine>,
    pub deployment: Arc<dyn DeploymentProvider>,
    pub metadata: Arc<dyn MetadataStore>,
    pub session: Arc<dyn SessionStore>,
    pub secrets: Arc<dyn SecretProvider>,
    pub audit: Arc<dyn AuditSink>,
    pub http: reqwest::Client,
    pub adapters: Arc<HashMap<String, ResolvedAdapter>>,
    pub started_at: std::time::Instant,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("identity", &self.identity.kind())
            .field("authz", &self.authz.kind())
            .field("deployment", &self.deployment.kind())
            .field("metadata", &self.metadata.kind())
            .field("session", &self.session.kind())
            .field("secrets", &self.secrets.kind())
            .field("audit", &self.audit.kind())
            .field("adapters", &self.adapters.len())
            .finish_non_exhaustive()
    }
}

impl AppState {
    pub async fn bootstrap(cfg: &Config) -> anyhow::Result<Self> {
        let identity = build_identity(cfg).await?;
        let authz = build_authz(cfg)?;
        let metadata = build_metadata(cfg).await?;
        let deployment = build_deployment(cfg).await?;

        let http = reqwest::Client::builder()
            .connect_timeout(cfg.upstream.connect_timeout())
            .timeout(cfg.upstream.request_timeout())
            .pool_max_idle_per_host(cfg.upstream.pool_max_idle_per_host)
            .user_agent(concat!("mcp-oxide/", env!("CARGO_PKG_VERSION")))
            .build()?;

        let mut adapter_map = HashMap::new();
        for a in &cfg.static_adapters {
            adapter_map.insert(a.name.clone(), ResolvedAdapter::from(a.clone()));
        }

        let s = Self {
            identity,
            authz,
            deployment,
            metadata,
            session: Arc::new(InMemorySessionStore::new()),
            secrets: Arc::new(EnvSecretProvider),
            audit: Arc::new(StdoutAuditSink),
            http,
            adapters: Arc::new(adapter_map),
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
            static_adapters = s.adapters.len(),
            "providers wired"
        );
        Ok(s)
    }

    /// Builder used by integration tests: supply pre-built providers +
    /// adapters directly, skip config parsing.
    #[must_use]
    pub fn builder() -> AppStateBuilder {
        AppStateBuilder::default()
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
            "static_adapters": self.adapters.len(),
        })
    }

    /// Resolve an adapter by name. Static config takes precedence; falls back
    /// to the `MetadataStore` so runtime-registered adapters are routable
    /// without a restart.
    pub async fn resolve_adapter(&self, name: &str) -> anyhow::Result<Option<ResolvedAdapter>> {
        if let Some(a) = self.adapters.get(name) {
            return Ok(Some(a.clone()));
        }
        if let Some(a) = self.metadata.get_adapter(name).await? {
            let Some(upstream) = a.upstream.clone() else {
                // Without DeploymentProvider (Phase 3) and no explicit
                // upstream URL, there is nowhere to route to.
                return Ok(None);
            };
            return Ok(Some(ResolvedAdapter {
                name: a.name,
                upstream,
                required_roles: a.required_roles,
                tags: a.tags,
            }));
        }
        Ok(None)
    }
}

#[derive(Default)]
pub struct AppStateBuilder {
    identity: Option<Arc<dyn IdProvider>>,
    authz: Option<Arc<dyn PolicyEngine>>,
    audit: Option<Arc<dyn AuditSink>>,
    adapters: Vec<ResolvedAdapter>,
}

impl std::fmt::Debug for AppStateBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppStateBuilder")
            .field("identity", &self.identity.as_ref().map(|i| i.kind()))
            .field("authz", &self.authz.as_ref().map(|a| a.kind()))
            .field("adapters", &self.adapters.len())
            .finish_non_exhaustive()
    }
}

impl AppStateBuilder {
    #[must_use]
    pub fn identity(mut self, p: Arc<dyn IdProvider>) -> Self {
        self.identity = Some(p);
        self
    }
    #[must_use]
    pub fn authz(mut self, p: Arc<dyn PolicyEngine>) -> Self {
        self.authz = Some(p);
        self
    }
    #[must_use]
    pub fn audit(mut self, p: Arc<dyn AuditSink>) -> Self {
        self.audit = Some(p);
        self
    }
    #[must_use]
    pub fn adapter(mut self, a: ResolvedAdapter) -> Self {
        self.adapters.push(a);
        self
    }

    pub fn build(self) -> anyhow::Result<AppState> {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let mut map = HashMap::new();
        for a in self.adapters {
            map.insert(a.name.clone(), a);
        }
        Ok(AppState {
            identity: self.identity.unwrap_or_else(|| {
                Arc::new(mcp_oxide_identity::NoopIdProvider) as Arc<dyn IdProvider>
            }),
            authz: self
                .authz
                .unwrap_or_else(|| Arc::new(DenyAllPolicyEngine) as Arc<dyn PolicyEngine>),
            deployment: Arc::new(NoopExternalProvider),
            metadata: Arc::new(InMemoryMetadataStore::new()),
            session: Arc::new(InMemorySessionStore::new()),
            secrets: Arc::new(EnvSecretProvider),
            audit: self
                .audit
                .unwrap_or_else(|| Arc::new(StdoutAuditSink) as Arc<dyn AuditSink>),
            http,
            adapters: Arc::new(map),
            started_at: std::time::Instant::now(),
        })
    }
}

async fn build_identity(cfg: &Config) -> anyhow::Result<Arc<dyn IdProvider>> {
    Ok(match &cfg.providers.identity {
        IdentityConfig::Noop => Arc::new(NoopIdProvider),
        IdentityConfig::OidcGeneric(o) => {
            let extractor = ClaimExtractor {
                role_paths: o.roles_claim_paths.clone(),
                group_paths: o.groups_claim_paths.clone(),
                tenant_path: o.tenant_claim_path.clone(),
                scopes_path: "scope".into(),
            };
            let algorithms = o
                .algorithms
                .iter()
                .map(|a| parse_alg(a))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let oc = OidcConfig {
                issuer: o.issuer.clone(),
                audiences: o.audiences.clone(),
                algorithms,
                jwks_cache_ttl: Duration::from_secs(o.jwks_cache_ttl_s),
                clock_skew_s: o.clock_skew_s,
                http_timeout: Duration::from_millis(o.http_timeout_ms),
                extractor,
            };
            let prov = OidcProvider::connect(oc).await?;
            Arc::new(prov)
        }
        IdentityConfig::StaticJwt(s) => {
            let alg = parse_alg(&s.algorithm)?;
            let key_bytes = std::fs::read(&s.key_path)?;
            let extractor = ClaimExtractor {
                role_paths: s.roles_claim_paths.clone(),
                group_paths: s.groups_claim_paths.clone(),
                tenant_path: s.tenant_claim_path.clone(),
                scopes_path: "scope".into(),
            };
            let decoding = match alg {
                Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 => {
                    jsonwebtoken::DecodingKey::from_rsa_pem(&key_bytes)?
                }
                Algorithm::ES256 | Algorithm::ES384 => {
                    jsonwebtoken::DecodingKey::from_ec_pem(&key_bytes)?
                }
                Algorithm::EdDSA => jsonwebtoken::DecodingKey::from_ed_pem(&key_bytes)?,
                Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
                    jsonwebtoken::DecodingKey::from_secret(&key_bytes)
                }
                other => anyhow::bail!("unsupported static-jwt algorithm: {other:?}"),
            };
            Arc::new(StaticJwtProvider::new(StaticJwtConfig {
                algorithm: alg,
                key: decoding,
                issuer: s.issuer.clone(),
                audiences: s.audiences.clone(),
                clock_skew_s: s.clock_skew_s,
                extractor,
            }))
        }
    })
}

fn parse_alg(s: &str) -> anyhow::Result<Algorithm> {
    Algorithm::from_str(s).map_err(|_| anyhow::anyhow!("invalid algorithm: {s}"))
}

fn build_authz(cfg: &Config) -> anyhow::Result<Arc<dyn PolicyEngine>> {
    Ok(match &cfg.providers.authz {
        AuthzConfig::DenyAll => Arc::new(DenyAllPolicyEngine),
        AuthzConfig::YamlRbac { path } => Arc::new(YamlRbacEngine::from_path(path)?),
    })
}

async fn build_metadata(cfg: &Config) -> anyhow::Result<Arc<dyn MetadataStore>> {
    Ok(match &cfg.providers.metadata_store {
        MetadataStoreConfig::InMemory => Arc::new(InMemoryMetadataStore::new()),
        MetadataStoreConfig::Sqlite { path } => {
            let url = if path.starts_with("sqlite:") {
                path.clone()
            } else {
                format!("sqlite://{path}?mode=rwc")
            };
            Arc::new(SqliteMetadataStore::connect(&url).await?)
        }
        MetadataStoreConfig::Postgres { .. } => {
            anyhow::bail!("postgres metadata store is not yet implemented (Phase 2 scope: sqlite)")
        }
    })
}

async fn build_deployment(cfg: &Config) -> anyhow::Result<Arc<dyn DeploymentProvider>> {
    Ok(match &cfg.providers.deployment {
        DeploymentConfig::NoopExternal => Arc::new(NoopExternalProvider),
        #[cfg(feature = "docker")]
        DeploymentConfig::Docker { socket, network } => {
            let config = mcp_oxide_deployment::DockerConfig {
                socket: socket.clone(),
                network: network.clone(),
            };
            Arc::new(DockerProvider::new(config).await?)
        }
        #[cfg(not(feature = "docker"))]
        DeploymentConfig::Docker { .. } => {
            anyhow::bail!("docker deployment provider requires 'docker' feature")
        }
    })
}
