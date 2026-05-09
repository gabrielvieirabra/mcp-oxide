//! Shared application state assembled from configured providers.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::Algorithm;
use moka::future::Cache;
use std::sync::atomic::{AtomicUsize, Ordering};
use mcp_oxide_audit::StdoutAuditSink;
use mcp_oxide_authz::{DenyAllPolicyEngine, YamlRbacEngine};
use mcp_oxide_core::providers::{
    AuditSink, DeploymentProvider, Endpoint as ProviderEndpoint, IdProvider, MetadataStore,
    PolicyEngine, SecretProvider, SessionStore,
};
use mcp_oxide_core::session::{BackendId, SessionId};
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

/// Cache key for tenant-scoped adapter/tool resolution.
type ResolveKey = (Option<String>, String);

/// TTL for the resolution caches. Short enough that an operator's
/// `MetadataStore` change made out-of-band (e.g. via raw SQL) propagates
/// within seconds; long enough to absorb the bulk of repeated lookups
/// from a busy data plane.
const RESOLVE_CACHE_TTL: Duration = Duration::from_secs(5);
const RESOLVE_CACHE_CAP: u64 = 10_000;

/// TTL applied when the data plane binds a session to a backend after
/// picking a fresh endpoint. Long enough that a typical MCP session stays
/// pinned through its lifetime; short enough that abandoned sessions don't
/// hold backend slots forever.
const SESSION_BIND_TTL: Duration = Duration::from_secs(30 * 60);

/// Cached resolution of a runtime-registered adapter: the `ResolvedAdapter`
/// metadata plus the current set of provider endpoints. Picking happens on
/// every request from the cached endpoint list, so multi-replica deployments
/// can round-robin or honor session affinity without re-hitting the
/// `DeploymentProvider`.
#[derive(Debug, Clone)]
pub struct ResolvedAdapterRecord {
    pub meta: ResolvedAdapter,
    pub endpoints: Arc<[ProviderEndpoint]>,
}

#[derive(Debug, Clone)]
pub struct ResolvedToolRecord {
    pub tool: mcp_oxide_core::tool::Tool,
    pub endpoints: Arc<[ProviderEndpoint]>,
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
    /// Tenant-scoped cache for runtime-registered adapters. Static adapters
    /// from config bypass this and are served from `adapters` directly.
    adapter_cache: Cache<ResolveKey, Arc<ResolvedAdapterRecord>>,
    /// Tenant-scoped cache for tool resolution.
    tool_cache: Cache<ResolveKey, Arc<ResolvedToolRecord>>,
    /// Per-target round-robin counter, used when no session id pins the
    /// caller to a specific backend.
    rr_counters: Arc<parking_lot::RwLock<HashMap<String, AtomicUsize>>>,
}

fn build_resolve_cache<V: Clone + Send + Sync + 'static>() -> Cache<ResolveKey, V> {
    Cache::builder()
        .max_capacity(RESOLVE_CACHE_CAP)
        .time_to_live(RESOLVE_CACHE_TTL)
        .build()
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

        // Start a background sweeper so expired session bindings are
        // reclaimed even when the affected session is never touched again
        // (e.g. clients that vanish without calling drop_session).
        let session = Arc::new(InMemorySessionStore::new());
        session.start_sweeper();

        let s = Self {
            identity,
            authz,
            deployment,
            metadata,
            session,
            secrets: Arc::new(EnvSecretProvider),
            audit: Arc::new(StdoutAuditSink),
            http,
            adapters: Arc::new(adapter_map),
            started_at: std::time::Instant::now(),
            adapter_cache: build_resolve_cache(),
            tool_cache: build_resolve_cache(),
            rr_counters: Arc::new(parking_lot::RwLock::new(HashMap::new())),
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

    /// Resolve an adapter by name within the caller's tenant. Static config
    /// adapters are tenant-less and visible to every authenticated caller;
    /// runtime-registered adapters are scoped strictly to the tenant that
    /// created them.
    ///
    /// Runtime resolutions are served from a short-TTL cache so the data
    /// plane doesn't hit `MetadataStore` + `DeploymentProvider` on every
    /// request. The control plane invalidates entries on mutation, so the
    /// TTL only bounds drift from out-of-band `MetadataStore` writes (e.g.
    /// raw SQL).
    pub async fn resolve_adapter(
        &self,
        name: &str,
        tenant: Option<&str>,
    ) -> anyhow::Result<Option<ResolvedAdapter>> {
        self.resolve_adapter_with_session(name, tenant, None).await
    }

    /// Tenant-scoped adapter resolution that honors session affinity. If
    /// `session_id` is `Some` and a binding exists in the `SessionStore`,
    /// the matching backend is preferred over the round-robin pick.
    pub async fn resolve_adapter_with_session(
        &self,
        name: &str,
        tenant: Option<&str>,
        session_id: Option<&SessionId>,
    ) -> anyhow::Result<Option<ResolvedAdapter>> {
        if let Some(a) = self.adapters.get(name) {
            return Ok(Some(a.clone()));
        }
        let record = self.adapter_record(name, tenant).await?;
        let Some(record) = record else { return Ok(None) };

        // Adapters with an explicit upstream (noop-external) only ever have
        // the synthetic single endpoint we minted at cache fill — pick is
        // a no-op. For provider-managed adapters with multiple endpoints
        // the picker chooses based on session affinity / round-robin.
        let chosen = self
            .pick_endpoint(&record.meta.name, &record.endpoints, session_id)
            .await;
        let Some(chosen) = chosen else {
            return Ok(None);
        };
        let mut out = record.meta.clone();
        out.upstream = chosen.url;
        Ok(Some(out))
    }

    async fn adapter_record(
        &self,
        name: &str,
        tenant: Option<&str>,
    ) -> anyhow::Result<Option<Arc<ResolvedAdapterRecord>>> {
        let key: ResolveKey = (tenant.map(ToString::to_string), name.to_string());
        if let Some(cached) = self.adapter_cache.get(&key).await {
            return Ok(Some(cached));
        }
        let Some(a) = self.metadata.get_adapter(name, tenant).await? else {
            return Ok(None);
        };
        let endpoints: Vec<ProviderEndpoint> = if let Some(ref upstream) = a.upstream {
            // Synthesize a single endpoint so the picker has something to
            // choose. The BackendId is derived from the URL so the same
            // session always gets the same binding.
            vec![ProviderEndpoint {
                url: upstream.clone(),
                backend_id: BackendId(format!("upstream:{upstream}")),
            }]
        } else {
            let handle = mcp_oxide_core::providers::DeploymentHandle {
                id: a.name.clone(),
                namespace: None,
                endpoint_url: None,
            };
            match self.deployment.endpoints(&handle).await {
                Ok(eps) if !eps.is_empty() => eps,
                _ => return Ok(None),
            }
        };
        let record = ResolvedAdapterRecord {
            meta: ResolvedAdapter {
                name: a.name,
                upstream: endpoints[0].url.clone(),
                required_roles: a.required_roles,
                tags: a.tags,
            },
            endpoints: Arc::from(endpoints.into_boxed_slice()),
        };
        let arc = Arc::new(record);
        self.adapter_cache.insert(key, arc.clone()).await;
        Ok(Some(arc))
    }

    pub async fn resolve_tool_endpoint(
        &self,
        name: &str,
        tenant: Option<&str>,
    ) -> anyhow::Result<Option<(mcp_oxide_core::tool::Tool, String)>> {
        self.resolve_tool_endpoint_with_session(name, tenant, None)
            .await
    }

    /// Tool-endpoint resolution honoring session affinity, with the same
    /// caching guarantees as adapters.
    pub async fn resolve_tool_endpoint_with_session(
        &self,
        name: &str,
        tenant: Option<&str>,
        session_id: Option<&SessionId>,
    ) -> anyhow::Result<Option<(mcp_oxide_core::tool::Tool, String)>> {
        let record = self.tool_record(name, tenant).await?;
        let Some(record) = record else { return Ok(None) };
        let Some(chosen) = self
            .pick_endpoint(&record.tool.name, &record.endpoints, session_id)
            .await
        else {
            return Ok(None);
        };
        Ok(Some((record.tool.clone(), chosen.url)))
    }

    async fn tool_record(
        &self,
        name: &str,
        tenant: Option<&str>,
    ) -> anyhow::Result<Option<Arc<ResolvedToolRecord>>> {
        let key: ResolveKey = (tenant.map(ToString::to_string), name.to_string());
        if let Some(cached) = self.tool_cache.get(&key).await {
            return Ok(Some(cached));
        }
        let Some(tool) = self.metadata.get_tool(name, tenant).await? else {
            return Ok(None);
        };
        let handle = mcp_oxide_core::providers::DeploymentHandle {
            id: tool.name.clone(),
            namespace: None,
            endpoint_url: None,
        };
        let endpoints = match self.deployment.endpoints(&handle).await {
            Ok(eps) if !eps.is_empty() => eps,
            _ => return Ok(None),
        };
        let record = ResolvedToolRecord {
            tool,
            endpoints: Arc::from(endpoints.into_boxed_slice()),
        };
        let arc = Arc::new(record);
        self.tool_cache.insert(key, arc.clone()).await;
        Ok(Some(arc))
    }

    /// Choose one endpoint from a non-empty list. Logic:
    /// * If exactly one endpoint exists, return it (no choice to make).
    /// * If `session_id` is supplied and the `SessionStore` already binds
    ///   it to a backend, prefer the endpoint whose `BackendId` matches.
    ///   On miss, the first round-robin pick is bound for the next call.
    /// * Otherwise round-robin across endpoints by target name.
    pub async fn pick_endpoint(
        &self,
        target: &str,
        endpoints: &[ProviderEndpoint],
        session_id: Option<&SessionId>,
    ) -> Option<ProviderEndpoint> {
        if endpoints.is_empty() {
            return None;
        }
        if endpoints.len() == 1 {
            // Still record the binding so a later replica scale-out preserves
            // the session's stickiness.
            if let Some(sid) = session_id {
                let _ = self
                    .session
                    .bind(
                        sid,
                        target,
                        endpoints[0].backend_id.clone(),
                        SESSION_BIND_TTL,
                    )
                    .await;
            }
            return Some(endpoints[0].clone());
        }
        if let Some(sid) = session_id {
            if let Ok(Some(bound)) = self.session.resolve(sid, target).await {
                if let Some(ep) = endpoints.iter().find(|e| e.backend_id == bound) {
                    return Some(ep.clone());
                }
                // Bound backend has gone away — fall through to fresh pick.
            }
            let chosen = self.round_robin_pick(target, endpoints);
            let _ = self
                .session
                .bind(
                    sid,
                    target,
                    chosen.backend_id.clone(),
                    SESSION_BIND_TTL,
                )
                .await;
            return Some(chosen);
        }
        Some(self.round_robin_pick(target, endpoints))
    }

    fn round_robin_pick(&self, target: &str, endpoints: &[ProviderEndpoint]) -> ProviderEndpoint {
        let idx = {
            let read = self.rr_counters.read();
            if let Some(c) = read.get(target) {
                c.fetch_add(1, Ordering::Relaxed)
            } else {
                drop(read);
                let mut write = self.rr_counters.write();
                write
                    .entry(target.to_string())
                    .or_insert_with(|| AtomicUsize::new(0))
                    .fetch_add(1, Ordering::Relaxed)
            }
        };
        endpoints[idx % endpoints.len()].clone()
    }

    /// Drop any cached resolution for the given adapter. Called by the
    /// control plane after a successful create/update/delete so subsequent
    /// data-plane traffic sees the new state immediately.
    pub async fn invalidate_adapter(&self, tenant: Option<&str>, name: &str) {
        let key: ResolveKey = (tenant.map(ToString::to_string), name.to_string());
        self.adapter_cache.invalidate(&key).await;
    }

    /// Drop any cached resolution for the given tool.
    pub async fn invalidate_tool(&self, tenant: Option<&str>, name: &str) {
        let key: ResolveKey = (tenant.map(ToString::to_string), name.to_string());
        self.tool_cache.invalidate(&key).await;
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
            adapter_cache: build_resolve_cache(),
            tool_cache: build_resolve_cache(),
            rr_counters: Arc::new(parking_lot::RwLock::new(HashMap::new())),
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

#[allow(clippy::unused_async)] // async only when the docker feature is on
async fn build_deployment(cfg: &Config) -> anyhow::Result<Arc<dyn DeploymentProvider>> {
    Ok(match &cfg.providers.deployment {
        DeploymentConfig::NoopExternal => Arc::new(NoopExternalProvider),
        #[cfg(feature = "docker")]
        DeploymentConfig::Docker {
            socket,
            network,
            connect_timeout_s,
            allowed_registries,
            require_digest_pinning,
        } => {
            let config = mcp_oxide_deployment::DockerConfig {
                socket: socket.clone(),
                network: network.clone(),
                connect_timeout_s: *connect_timeout_s,
                allowed_registries: allowed_registries.clone(),
                require_digest_pinning: *require_digest_pinning,
            };
            Arc::new(DockerProvider::new(config).await?)
        }
        #[cfg(not(feature = "docker"))]
        DeploymentConfig::Docker { .. } => {
            anyhow::bail!("docker deployment provider requires 'docker' feature")
        }
    })
}
