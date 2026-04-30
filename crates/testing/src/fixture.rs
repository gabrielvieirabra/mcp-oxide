//! Fixture types that describe mock MCP behaviour.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// Per-tool fixture driving a `tools/call` dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFixture {
    /// Optional title advertised in `tools/list`.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional description advertised in `tools/list`.
    #[serde(default)]
    pub description: Option<String>,
    /// Input JSON Schema advertised in `tools/list`. Defaults to
    /// `{"type": "object"}` when unset.
    #[serde(default = "default_schema")]
    pub input_schema: Value,
    /// Tool annotations advertised in `tools/list`.
    #[serde(default)]
    pub annotations: Option<Value>,
    /// Response body returned as the JSON-RPC `result` on a successful
    /// `tools/call`. When unset the tool echoes the incoming params.
    #[serde(default)]
    pub result: Option<Value>,
    /// If set, the tool fails its `tools/call` with a JSON-RPC error object
    /// of the given code and message. Takes precedence over `result`.
    #[serde(default)]
    pub fail_with: Option<ToolError>,
    /// Per-tool latency injection. Added on top of the global
    /// `MockFixture::latency`.
    #[serde(default)]
    #[serde(with = "duration_ms_opt")]
    pub latency: Option<Duration>,
}

impl Default for ToolFixture {
    fn default() -> Self {
        Self {
            title: None,
            description: None,
            input_schema: default_schema(),
            annotations: None,
            result: None,
            fail_with: None,
            latency: None,
        }
    }
}

fn default_schema() -> Value {
    serde_json::json!({ "type": "object" })
}

/// A programmable JSON-RPC error returned from a mocked `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// Fault injection options that apply to every request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultInjection {
    /// If set, every response is delayed by this amount.
    #[serde(default)]
    #[serde(with = "duration_ms_opt")]
    pub latency: Option<Duration>,
    /// If set, every HTTP response carries this status code instead of 200.
    /// Useful for asserting the gateway's upstream-error path.
    #[serde(default)]
    pub force_status: Option<u16>,
    /// If set, the response body is a non-JSON blob of the given size. Used
    /// to ensure clients tolerate malformed upstream responses.
    #[serde(default)]
    pub bogus_body_bytes: Option<usize>,
    /// Probability 0-100 that a request is dropped (connection closed after
    /// reading the body, no response sent). 0 = never.
    #[serde(default)]
    pub drop_percent: u8,
    /// If true, the mock records the `Authorization` request header. Used to
    /// assert that the gateway does NOT forward client tokens upstream.
    #[serde(default = "yes")]
    pub record_auth_header: bool,
}

impl Default for FaultInjection {
    fn default() -> Self {
        Self {
            latency: None,
            force_status: None,
            bogus_body_bytes: None,
            drop_percent: 0,
            record_auth_header: true,
        }
    }
}

fn yes() -> bool {
    true
}

/// Top-level mock configuration. Deserializable from the YAML fixtures that
/// back the `mock-mcp` binary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MockFixture {
    /// Server name surfaced in logs (and optionally as the initialization
    /// response's server.name).
    #[serde(default)]
    pub name: Option<String>,
    /// Tools advertised via `tools/list` and dispatchable via `tools/call`.
    #[serde(default)]
    pub tools: BTreeMap<String, ToolFixture>,
    /// Global fault injection, evaluated before the per-tool fixture.
    #[serde(default)]
    pub fault: FaultInjection,
}

impl MockFixture {
    /// Load a fixture from a YAML file.
    pub fn from_yaml_path(path: &std::path::Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        let f: Self = serde_yaml::from_str(&s)?;
        Ok(f)
    }
}

mod duration_ms_opt {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    #[allow(clippy::ref_option)] // serde signature is fixed
    pub fn serialize<S: Serializer>(v: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(d) => {
                let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
                ms.serialize(s)
            }
            None => Option::<u64>::None.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        let v = Option::<u64>::deserialize(d)?;
        Ok(v.map(Duration::from_millis))
    }
}
