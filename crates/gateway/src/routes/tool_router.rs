//! Tool Gateway Router — in-process MCP server that aggregates tools.
//!
//! Exposed at `POST /mcp`. Aggregates `tools/list` across registered tools
//! (filtered by per-user policy) and dispatches `tools/call` to the right
//! backend. Both operations are authorized against the policy engine and
//! audited.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
};
use mcp_oxide_core::{
    audit::AuditDecision,
    identity::UserContext,
    policy::ResourceKind,
    tool::Tool,
    Error, Result,
};
use mcp_oxide_mcp::jsonrpc::{ErrorObject, Request as JsonRequest, Response as JsonResponse};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Instant;
use tracing::debug;

use crate::{
    auth::AuthUser,
    error::AppError,
    routes::data_plane_helpers::{authorize_data_plane, emit_audit, extract_trace_id},
    state::AppState,
};

// JSON-RPC error codes per PLAN.md §2.5.
const JSONRPC_FORBIDDEN: i32 = -32002;
const JSONRPC_NOT_FOUND: i32 = -32601;
const JSONRPC_INVALID_PARAMS: i32 = -32602;
const JSONRPC_INTERNAL: i32 = -32603;
const JSONRPC_UPSTREAM_UNAVAILABLE: i32 = -32004;
const JSONRPC_UPSTREAM_TIMEOUT: i32 = -32005;

/// Hard cap for `tools/list` page size regardless of client request.
const MAX_TOOLS_PAGE_SIZE: usize = 200;
/// Default page size when the client doesn't ask for one.
const DEFAULT_TOOLS_PAGE_SIZE: usize = 50;

/// Handle `POST /mcp` — Tool Gateway Router.
pub async fn invoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    req: Request<Body>,
) -> Result<Response, AppError> {
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|e| Error::InvalidRequest(format!("read body: {e}")))?;

    let trace_id = extract_trace_id(&parts.headers);
    let rpc: JsonRequest = serde_json::from_slice(&body).map_err(|e| {
        Error::InvalidRequest(format!("parse json-rpc: {e}"))
    })?;

    debug!(method = %rpc.method, id = ?rpc.id, "tool router request");

    let response = match rpc.method.as_str() {
        "tools/list" => handle_tools_list(&state, &user, &trace_id, &rpc).await,
        "tools/call" => handle_tools_call(&state, &user, &trace_id, &rpc, &body).await,
        "ping" => Ok(JsonResponse {
            jsonrpc: "2.0".into(),
            id: rpc.id.clone(),
            result: Some(json!({})),
            error: None,
        }),
        _ => Ok(JsonResponse {
            jsonrpc: "2.0".into(),
            id: rpc.id.clone(),
            result: None,
            error: Some(ErrorObject {
                code: JSONRPC_NOT_FOUND,
                message: "method not found".into(),
                data: None,
            }),
        }),
    };

    let resp = match response {
        Ok(resp) => resp,
        Err(e) => JsonResponse {
            jsonrpc: "2.0".into(),
            id: rpc.id.clone(),
            result: None,
            error: Some(map_error_to_jsonrpc(&e)),
        },
    };

    let body = serde_json::to_vec(&resp).map_err(|e| Error::Internal(format!("serialize: {e}")))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .map_err(|e| Error::Internal(format!("build response: {e}")))?)
}

fn map_error_to_jsonrpc(e: &Error) -> ErrorObject {
    let (code, msg) = match e {
        Error::Forbidden(m) => (JSONRPC_FORBIDDEN, m.as_str()),
        Error::NotFound(m) => (JSONRPC_NOT_FOUND, m.as_str()),
        Error::InvalidRequest(m) => (JSONRPC_INVALID_PARAMS, m.as_str()),
        Error::UpstreamUnavailable(m) => (JSONRPC_UPSTREAM_UNAVAILABLE, m.as_str()),
        Error::UpstreamTimeout(m) => (JSONRPC_UPSTREAM_TIMEOUT, m.as_str()),
        _ => (JSONRPC_INTERNAL, "internal_error"),
    };
    ErrorObject {
        code,
        message: msg.into(),
        data: None,
    }
}

#[derive(Debug, serde::Deserialize, Default)]
struct ListParams {
    /// MCP pagination cursor; opaque to clients.
    #[serde(default)]
    cursor: Option<String>,
    /// Optional client-requested page size. Capped at `MAX_TOOLS_PAGE_SIZE`.
    #[serde(default)]
    #[serde(rename = "pageSize")]
    page_size: Option<usize>,
}

#[allow(clippy::too_many_lines)]
async fn handle_tools_list(
    state: &AppState,
    user: &UserContext,
    trace_id: &str,
    rpc: &JsonRequest,
) -> Result<JsonResponse> {
    // AuthZ on the list operation itself.
    let authz = match authorize_data_plane(
        state,
        user,
        "tools/list",
        None,
        ResourceKind::Tool,
        "*",
        &[],
        &[],
    )
    .await
    {
        Ok(a) => a,
        Err(reason) => {
            emit_audit(
                state,
                trace_id,
                user,
                "tools/list",
                "tool",
                "*",
                AuditDecision::Deny,
                None,
                0,
                "forbidden",
                "",
                Some(&reason),
            )
            .await;
            return Err(Error::Forbidden(reason));
        }
    };

    let params: ListParams = rpc
        .params
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| Error::InvalidRequest(format!("tools/list params: {e}")))?
        .unwrap_or_default();
    let page_size = params
        .page_size
        .unwrap_or(DEFAULT_TOOLS_PAGE_SIZE)
        .clamp(1, MAX_TOOLS_PAGE_SIZE);
    let cursor_start: usize = params
        .cursor
        .as_deref()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);

    let all = state
        .metadata
        .list_tools(&mcp_oxide_core::providers::Filter::default())
        .await?;

    // B3: filter by per-tool visibility. A tool the caller cannot call should
    // not appear in their tools/list (information disclosure).
    let mut visible: Vec<&Tool> = Vec::with_capacity(all.len());
    for t in &all {
        if can_call_tool(state, user, t).await {
            visible.push(t);
        }
    }

    let end = (cursor_start + page_size).min(visible.len());
    let page = &visible[cursor_start.min(visible.len())..end];
    let tool_defs: Vec<serde_json::Value> = page
        .iter()
        .map(|t| {
            json!({
                "name": t.tool_definition.name,
                "title": t.tool_definition.title,
                "description": t.tool_definition.description,
                "inputSchema": t.tool_definition.input_schema,
                "annotations": t.tool_definition.annotations,
            })
        })
        .collect();

    let next_cursor = if end < visible.len() {
        Some(end.to_string())
    } else {
        None
    };

    emit_audit(
        state,
        trace_id,
        user,
        "tools/list",
        "tool",
        "*",
        AuditDecision::Allow,
        authz.policy_id,
        0,
        "ok",
        "",
        None,
    )
    .await;

    let mut result = serde_json::Map::new();
    result.insert("tools".into(), json!(tool_defs));
    if let Some(c) = next_cursor {
        result.insert("nextCursor".into(), json!(c));
    }

    Ok(JsonResponse {
        jsonrpc: "2.0".into(),
        id: rpc.id.clone(),
        result: Some(serde_json::Value::Object(result)),
        error: None,
    })
}

/// Cheap read-only authz check used to filter `tools/list` (no audit emit).
async fn can_call_tool(state: &AppState, user: &UserContext, tool: &Tool) -> bool {
    authorize_data_plane(
        state,
        user,
        "tools/call",
        Some(&tool.tool_definition.name),
        ResourceKind::Tool,
        &tool.name,
        &tool.tags,
        &tool.required_roles,
    )
    .await
    .is_ok()
}

#[allow(clippy::too_many_lines)]
async fn handle_tools_call(
    state: &AppState,
    user: &UserContext,
    trace_id: &str,
    rpc: &JsonRequest,
    raw_body: &[u8],
) -> Result<JsonResponse> {
    let params = rpc.params.clone().ok_or_else(|| {
        Error::InvalidRequest("tools/call requires params".into())
    })?;
    let tool_name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::InvalidRequest("missing tool name".into()))?
        .to_string();

    let request_hash = format!("sha256:{}", hex::encode(Sha256::digest(raw_body)));

    // Look up tool metadata first (needed for authz tags + required_roles).
    // Authz must run BEFORE the deployment-endpoint lookup so an
    // unauthorized caller can't use response-code differences to probe for
    // tool existence.
    let tool = match state.metadata.get_tool(&tool_name).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            emit_audit(
                state,
                trace_id,
                user,
                "tools/call",
                "tool",
                &tool_name,
                AuditDecision::Deny,
                None,
                0,
                "not_found",
                &request_hash,
                Some("tool not found"),
            )
            .await;
            return Err(Error::NotFound(format!("tool '{tool_name}'")));
        }
        Err(e) => return Err(Error::Internal(e.to_string())),
    };

    // S1: authz on the tool itself, using tags + required_roles.
    let authz = match authorize_data_plane(
        state,
        user,
        "tools/call",
        Some(&tool.tool_definition.name),
        ResourceKind::Tool,
        &tool.name,
        &tool.tags,
        &tool.required_roles,
    )
    .await
    {
        Ok(a) => a,
        Err(reason) => {
            emit_audit(
                state,
                trace_id,
                user,
                "tools/call",
                "tool",
                &tool.name,
                AuditDecision::Deny,
                None,
                0,
                "forbidden",
                &request_hash,
                Some(&reason),
            )
            .await;
            return Err(Error::Forbidden(reason));
        }
    };

    // Resolve the deployment endpoint. If the caller is authorized but no
    // endpoint is ready, return UpstreamUnavailable rather than a spurious
    // NotFound.
    let endpoint_url = match state.deployment.endpoints(&crate::routes::control_plane_helpers::handle_for(&tool.name)).await {
        Ok(eps) if !eps.is_empty() => eps.into_iter().next().unwrap().url,
        _ => {
            let msg = "no ready endpoint for tool".to_string();
            emit_audit(
                state,
                trace_id,
                user,
                "tools/call",
                "tool",
                &tool.name,
                AuditDecision::Allow,
                authz.policy_id.clone(),
                0,
                "upstream_unavailable",
                &request_hash,
                Some(&msg),
            )
            .await;
            return Err(Error::UpstreamUnavailable(msg));
        }
    };

    // Forward.
    let forward_req = JsonRequest {
        jsonrpc: "2.0".into(),
        id: rpc.id.clone(),
        method: "tools/call".into(),
        params: Some(params),
    };

    let start = Instant::now();
    let outcome = state
        .http
        .post(&endpoint_url)
        .json(&forward_req)
        .send()
        .await;
    let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    let (response_body, upstream_status, err_for_audit): (
        serde_json::Value,
        &'static str,
        Option<String>,
    ) = match outcome {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r
                .json()
                .await
                .map_err(|e| Error::Internal(format!("parse response: {e}")))?;
            (body, "ok", None)
        }
        Ok(r) => {
            let status = r.status();
            let msg = format!("tool backend returned {status}");
            let label = if status.is_server_error() {
                "upstream_error"
            } else {
                "upstream_client_error"
            };
            emit_audit(
                state,
                trace_id,
                user,
                "tools/call",
                "tool",
                &tool.name,
                AuditDecision::Allow,
                authz.policy_id.clone(),
                latency_ms,
                label,
                &request_hash,
                Some(&msg),
            )
            .await;
            return Err(Error::UpstreamUnavailable(msg));
        }
        Err(e) if e.is_timeout() => {
            let msg = format!("tool backend timeout: {e}");
            emit_audit(
                state,
                trace_id,
                user,
                "tools/call",
                "tool",
                &tool.name,
                AuditDecision::Allow,
                authz.policy_id.clone(),
                latency_ms,
                "upstream_timeout",
                &request_hash,
                Some(&msg),
            )
            .await;
            return Err(Error::UpstreamTimeout(msg));
        }
        Err(e) => {
            let msg = format!("tool backend: {e}");
            emit_audit(
                state,
                trace_id,
                user,
                "tools/call",
                "tool",
                &tool.name,
                AuditDecision::Allow,
                authz.policy_id.clone(),
                latency_ms,
                "upstream_unavailable",
                &request_hash,
                Some(&msg),
            )
            .await;
            return Err(Error::UpstreamUnavailable(msg));
        }
    };

    emit_audit(
        state,
        trace_id,
        user,
        "tools/call",
        "tool",
        &tool.name,
        AuditDecision::Allow,
        authz.policy_id,
        latency_ms,
        upstream_status,
        &request_hash,
        err_for_audit.as_deref(),
    )
    .await;

    // The upstream JSON-RPC response already contains jsonrpc/id/result —
    // preserve it verbatim so clients see the tool's response shape.
    if let Ok(upstream_rpc) = serde_json::from_value::<JsonResponse>(response_body.clone()) {
        return Ok(upstream_rpc);
    }
    // Fall back: treat the body as the `result` field.
    Ok(JsonResponse {
        jsonrpc: "2.0".into(),
        id: rpc.id.clone(),
        result: Some(response_body),
        error: None,
    })
}
