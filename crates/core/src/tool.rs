//! Registered Tool domain model.

use crate::adapter::{Endpoint, EnvVar, ImageRef, Resources, SecretRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub image: ImageRef,
    pub endpoint: Endpoint,
    pub tool_definition: ToolDefinition,
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
}

/// MCP tool definition (subset aligned with `tools/call` input).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Value>,
}
