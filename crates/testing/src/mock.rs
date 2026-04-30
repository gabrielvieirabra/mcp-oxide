//! The mock MCP server itself.

#![allow(
    // Test-harness code: clarity trumps pedantic micro-optimizations.
    clippy::needless_pass_by_value,
    clippy::ref_option,
    clippy::missing_fields_in_debug,
    clippy::single_match_else,
    clippy::cast_possible_truncation,
    clippy::match_wild_err_arm,
    clippy::manual_let_else,
)]

use crate::fixture::{MockFixture, ToolError, ToolFixture};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, info};

/// A captured request for later assertions.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub id: Option<Value>,
    pub params: Option<Value>,
    /// Raw `Authorization` header if `FaultInjection::record_auth_header`
    /// is true. Serves as evidence that the gateway did or did not forward
    /// the caller's token upstream.
    pub authorization: Option<String>,
}

/// A running mock MCP server. Drop stops the server.
pub struct MockMcp {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<()>>,
    recorder: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockMcp {
    /// Programmatic builder.
    #[must_use]
    pub fn builder() -> MockMcpBuilder {
        MockMcpBuilder::default()
    }

    /// `http://127.0.0.1:<port>` — the URL the gateway should be pointed at.
    /// Does NOT include the `/mcp` suffix.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Full URL including `/mcp`.
    #[must_use]
    pub fn mcp_url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns a snapshot of all requests received so far.
    #[must_use]
    pub fn recorded(&self) -> Vec<RecordedRequest> {
        self.recorder.lock().clone()
    }

    /// Number of requests received.
    #[must_use]
    pub fn recorded_count(&self) -> usize {
        self.recorder.lock().len()
    }

    /// Cleanly shut the server down. Called automatically on `Drop`.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
    }
}

impl Drop for MockMcp {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl std::fmt::Debug for MockMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockMcp")
            .field("addr", &self.addr)
            .field("recorded", &self.recorded_count())
            .finish()
    }
}

/// Programmatic builder for [`MockMcp`].
#[derive(Debug, Default)]
pub struct MockMcpBuilder {
    fixture: MockFixture,
    bind: Option<SocketAddr>,
}

impl MockMcpBuilder {
    /// Register a tool that echoes the incoming params unchanged.
    #[must_use]
    pub fn echo_tool(mut self, name: impl Into<String>) -> Self {
        self.fixture
            .tools
            .insert(name.into(), ToolFixture::default());
        self
    }

    /// Register a tool that returns a fixed JSON result on `tools/call`.
    #[must_use]
    pub fn tool(mut self, name: impl Into<String>, result: Value) -> Self {
        let fx = ToolFixture {
            result: Some(result),
            ..Default::default()
        };
        self.fixture.tools.insert(name.into(), fx);
        self
    }

    /// Register a tool that always returns a JSON-RPC error.
    #[must_use]
    pub fn failing_tool(mut self, name: impl Into<String>, err: ToolError) -> Self {
        let fx = ToolFixture {
            fail_with: Some(err),
            ..Default::default()
        };
        self.fixture.tools.insert(name.into(), fx);
        self
    }

    /// Replace the whole fixture (useful when loading from YAML).
    #[must_use]
    pub fn fixture(mut self, fx: MockFixture) -> Self {
        self.fixture = fx;
        self
    }

    /// Inject global latency on every response.
    #[must_use]
    pub fn latency(mut self, d: std::time::Duration) -> Self {
        self.fixture.fault.latency = Some(d);
        self
    }

    /// Force every response to carry this HTTP status.
    #[must_use]
    pub fn force_status(mut self, code: u16) -> Self {
        self.fixture.fault.force_status = Some(code);
        self
    }

    /// Bind to a specific address. Defaults to `127.0.0.1:0`.
    #[must_use]
    pub fn bind(mut self, addr: SocketAddr) -> Self {
        self.bind = Some(addr);
        self
    }

    /// Start the server. Returns once it is accepting connections.
    pub async fn build(self) -> anyhow::Result<MockMcp> {
        let recorder: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let state = AppState {
            fixture: Arc::new(self.fixture),
            recorder: recorder.clone(),
        };

        let app = Router::new()
            .route("/mcp", post(handle_mcp))
            .route("/healthz", get(|| async { "ok" }))
            .with_state(state);

        let addr = self
            .bind
            .unwrap_or_else(|| "127.0.0.1:0".parse().expect("valid addr"));
        let listener = TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        info!(addr = %bound, "mock-mcp listening");

        let (tx, rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let server = axum::serve(listener, app);
            let _ = server
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });

        // Tiny yield so the listener is ready before the caller starts
        // hitting the URL. `bind` already guarantees ready; this is cheap.
        tokio::task::yield_now().await;

        Ok(MockMcp {
            addr: bound,
            shutdown: Some(tx),
            handle: Some(handle),
            recorder,
        })
    }
}

#[derive(Clone)]
struct AppState {
    fixture: Arc<MockFixture>,
    recorder: Arc<Mutex<Vec<RecordedRequest>>>,
}

async fn handle_mcp(State(state): State<AppState>, req: Request<Body>) -> Response {
    let (parts, body) = req.into_parts();
    let headers = parts.headers;

    let body_bytes = match axum::body::to_bytes(body, 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "read body"),
    };

    let rpc: Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "bad json"),
    };    let method = rpc
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let id = rpc.get("id").cloned();
    let params = rpc.get("params").cloned();

    record(&state, &headers, &method, id.clone(), params.clone());

    let fault = &state.fixture.fault;
    if let Some(d) = fault.latency {
        tokio::time::sleep(d).await;
    }

    // Deterministic drop: first check percent, then proceed.
    if fault.drop_percent > 0 {
        let roll = fastrand_u8() % 100;
        if roll < fault.drop_percent {
            debug!(method = %method, "mock dropping connection");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "dropped");
        }
    }

    if let Some(size) = fault.bogus_body_bytes {
        let payload = vec![b'x'; size];
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/octet-stream")
            .body(Body::from(payload))
            .expect("valid response");
    }

    let resp_body: Value = match method.as_str() {
        "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        "tools/list" => handle_tools_list(&state, id.clone()),
        "tools/call" => handle_tools_call(&state, id.clone(), params).await,
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": state.fixture.name.clone().unwrap_or_else(|| "mock-mcp".into()),
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }
        }),
        other => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("method not found: {other}") }
        }),
    };

    let status_code = fault.force_status.unwrap_or(200);
    let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::OK);
    let body = match serde_json::to_vec(&resp_body) {
        Ok(b) => b,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "serialize"),
    };
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("valid response")
}

fn record(
    state: &AppState,
    headers: &HeaderMap,
    method: &str,
    id: Option<Value>,
    params: Option<Value>,
) {
    let authorization = if state.fixture.fault.record_auth_header {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned)
    } else {
        None
    };
    state.recorder.lock().push(RecordedRequest {
        method: method.to_string(),
        id,
        params,
        authorization,
    });
}

fn handle_tools_list(state: &AppState, id: Option<Value>) -> Value {
    let tools: Vec<Value> = state
        .fixture
        .tools
        .iter()
        .map(|(name, fx)| {
            let mut obj = serde_json::Map::new();
            obj.insert("name".into(), json!(name));
            if let Some(t) = &fx.title {
                obj.insert("title".into(), json!(t));
            }
            if let Some(d) = &fx.description {
                obj.insert("description".into(), json!(d));
            }
            obj.insert("inputSchema".into(), fx.input_schema.clone());
            if let Some(a) = &fx.annotations {
                obj.insert("annotations".into(), a.clone());
            }
            Value::Object(obj)
        })
        .collect();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "tools": tools }
    })
}

async fn handle_tools_call(state: &AppState, id: Option<Value>, params: Option<Value>) -> Value {
    let params = params.unwrap_or(json!({}));
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(fx) = state.fixture.tools.get(name) else {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("unknown tool: {name}") }
        });
    };

    if let Some(d) = fx.latency {
        tokio::time::sleep(d).await;
    }

    if let Some(err) = &fx.fail_with {
        let mut obj = serde_json::Map::new();
        obj.insert("code".into(), json!(err.code));
        obj.insert("message".into(), json!(err.message.clone()));
        if let Some(d) = &err.data {
            obj.insert("data".into(), d.clone());
        }
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": Value::Object(obj)
        });
    }

    let result = fx
        .result
        .clone()
        .unwrap_or_else(|| json!({ "echo": params }));
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn error_response(code: StatusCode, msg: &str) -> Response {
    Response::builder()
        .status(code)
        .body(Body::from(format!("{{\"error\":\"{msg}\"}}")))
        .expect("valid response")
}

/// Tiny PRNG to avoid pulling `rand` just for fault injection.
fn fastrand_u8() -> u8 {
    use std::cell::Cell;
    use std::time::SystemTime;
    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0x9E37_79B9_7F4A_7C15) };
    }
    STATE.with(|s| {
        let mut x = s.get();
        if x == 0 {
            x = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x1);
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        (x & 0xFF) as u8
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn list_and_call_echo() {
        let mock = MockMcp::builder().echo_tool("echo").build().await.unwrap();

        let client = reqwest::Client::new();
        let url = mock.mcp_url();

        // tools/list
        let list: Value = client
            .post(&url)
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["echo"]);

        // tools/call → echoes
        let call: Value = client
            .post(&url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": "echo", "arguments": { "x": 1 } }
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(call["id"], 2);
        assert_eq!(call["result"]["echo"]["name"], "echo");
        assert_eq!(call["result"]["echo"]["arguments"]["x"], 1);

        // Recorder observed both requests.
        let recs = mock.recorded();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].method, "tools/list");
        assert_eq!(recs[1].method, "tools/call");
    }

    #[tokio::test]
    async fn fixed_result_and_failing_tool() {
        let mock = MockMcp::builder()
            .tool("weather", json!({ "forecast": "sunny" }))
            .failing_tool(
                "broken",
                ToolError {
                    code: -32000,
                    message: "kaboom".into(),
                    data: None,
                },
            )
            .build()
            .await
            .unwrap();

        let client = reqwest::Client::new();
        let url = mock.mcp_url();

        let ok: Value = client
            .post(&url)
            .json(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "weather" }
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(ok["result"]["forecast"], "sunny");

        let err: Value = client
            .post(&url)
            .json(&json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "broken" }
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(err["error"]["code"], -32000);
        assert_eq!(err["error"]["message"], "kaboom");
    }

    #[tokio::test]
    async fn force_status_for_upstream_error_tests() {
        let mock = MockMcp::builder()
            .echo_tool("x")
            .force_status(503)
            .build()
            .await
            .unwrap();
        let status = reqwest::Client::new()
            .post(mock.mcp_url())
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status.as_u16(), 503);
    }

    #[tokio::test]
    async fn records_auth_header_by_default() {
        let mock = MockMcp::builder().echo_tool("e").build().await.unwrap();
        let _ = reqwest::Client::new()
            .post(mock.mcp_url())
            .bearer_auth("test-token")
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
            .send()
            .await
            .unwrap();
        let rec = &mock.recorded()[0];
        assert_eq!(rec.authorization.as_deref(), Some("Bearer test-token"));
    }
}
