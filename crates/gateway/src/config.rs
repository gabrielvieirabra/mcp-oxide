//! Gateway configuration (loaded from file + env).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub logs: LogConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub upstream: UpstreamConfig,
    #[serde(default)]
    pub static_adapters: Vec<StaticAdapter>,
}

// -------- Server -----------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)] // timeout + body limit enforced in Phase 4 (Protection)
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_body_limit")]
    pub max_body_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            request_timeout_ms: default_request_timeout_ms(),
            max_body_bytes: default_body_limit(),
        }
    }
}

fn default_bind() -> SocketAddr {
    "0.0.0.0:8080".parse().unwrap()
}
fn default_request_timeout_ms() -> u64 {
    30_000
}
fn default_body_limit() -> usize {
    1024 * 1024
}

// -------- Logs -------------------------------------------------------------

#[derive(Debug, Deserialize, Clone, Default)]
pub struct LogConfig {
    #[serde(default)]
    pub json: bool,
}

// -------- Providers --------------------------------------------------------

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub identity: IdentityConfig,
    #[serde(default)]
    pub authz: AuthzConfig,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum IdentityConfig {
    #[default]
    Noop,
    OidcGeneric(OidcGenericConfig),
    StaticJwt(StaticJwtFileConfig),
}

#[derive(Debug, Deserialize, Clone)]
pub struct OidcGenericConfig {
    pub issuer: String,
    #[serde(default)]
    pub audiences: Vec<String>,
    #[serde(default = "default_jwks_ttl")]
    pub jwks_cache_ttl_s: u64,
    #[serde(default = "default_clock_skew")]
    pub clock_skew_s: u64,
    #[serde(default = "default_algorithms")]
    pub algorithms: Vec<String>,
    #[serde(default = "default_role_paths")]
    pub roles_claim_paths: Vec<String>,
    #[serde(default = "default_group_paths")]
    pub groups_claim_paths: Vec<String>,
    #[serde(default = "default_tenant_path")]
    pub tenant_claim_path: Option<String>,
    #[serde(default = "default_http_timeout_ms")]
    pub http_timeout_ms: u64,
}

fn default_jwks_ttl() -> u64 {
    300
}
fn default_clock_skew() -> u64 {
    30
}
fn default_algorithms() -> Vec<String> {
    vec!["RS256".into(), "ES256".into()]
}
fn default_role_paths() -> Vec<String> {
    vec!["realm_access.roles".into(), "roles".into()]
}
fn default_group_paths() -> Vec<String> {
    vec!["groups".into()]
}
#[allow(clippy::unnecessary_wraps)]
fn default_tenant_path() -> Option<String> {
    Some("tenant".into())
}
fn default_http_timeout_ms() -> u64 {
    5_000
}

#[derive(Debug, Deserialize, Clone)]
pub struct StaticJwtFileConfig {
    pub algorithm: String,
    /// Path to a PEM public key, or (for HS algs) to a file with raw bytes.
    pub key_path: PathBuf,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub audiences: Vec<String>,
    #[serde(default = "default_clock_skew")]
    pub clock_skew_s: u64,
    #[serde(default = "default_role_paths")]
    pub roles_claim_paths: Vec<String>,
    #[serde(default = "default_group_paths")]
    pub groups_claim_paths: Vec<String>,
    #[serde(default = "default_tenant_path")]
    pub tenant_claim_path: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AuthzConfig {
    #[default]
    DenyAll,
    YamlRbac {
        path: PathBuf,
    },
}

// -------- Upstream ---------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct UpstreamConfig {
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_upstream_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_pool_idle")]
    pub pool_max_idle_per_host: usize,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            connect_timeout_ms: default_connect_timeout_ms(),
            request_timeout_ms: default_upstream_timeout_ms(),
            pool_max_idle_per_host: default_pool_idle(),
        }
    }
}

fn default_connect_timeout_ms() -> u64 {
    2_000
}
fn default_upstream_timeout_ms() -> u64 {
    25_000
}
fn default_pool_idle() -> usize {
    32
}

impl UpstreamConfig {
    #[must_use]
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }
    #[must_use]
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }
}

// -------- Static adapters --------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct StaticAdapter {
    pub name: String,
    pub upstream: String,
    #[serde(default)]
    pub required_roles: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

// -------- Loader -----------------------------------------------------------

impl Config {
    /// Load config from `MCP_OXIDE_CONFIG` (file path, optional) merged with
    /// `MCP_OXIDE__…` env vars. Falls back to defaults.
    pub fn load() -> anyhow::Result<Self> {
        let mut fig = Figment::new();
        if let Ok(path) = std::env::var("MCP_OXIDE_CONFIG") {
            fig = fig.merge(Yaml::file(path));
        }
        let fig = fig.merge(Env::prefixed("MCP_OXIDE__").split("__"));
        Ok(fig.extract()?)
    }
}
