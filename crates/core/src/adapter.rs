//! Registered MCP server (Adapter) domain model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adapter {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub image: ImageRef,
    pub endpoint: Endpoint,
    /// Optional explicit upstream URL. When set, the data plane proxies to
    /// this URL directly. When unset, the `DeploymentProvider` (Phase 3+) is
    /// queried for endpoints. Useful for `noop-external` mode where the
    /// adapter is already running somewhere reachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default = "one")]
    pub replicas: u32,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub secret_refs: Vec<SecretRef>,
    #[serde(default)]
    pub required_roles: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub resources: Resources,
    #[serde(default)]
    pub health: Option<HealthProbe>,
    #[serde(default)]
    pub session_affinity: SessionAffinity,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRef {
    /// Fully-qualified OCI reference (registry/repo:tag[@digest]).
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub port: u16,
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_path() -> String {
    "/mcp".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretRef {
    pub name: String,
    pub provider: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Resources {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthProbe {
    pub path: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionAffinity {
    #[default]
    Sticky,
    None,
}
