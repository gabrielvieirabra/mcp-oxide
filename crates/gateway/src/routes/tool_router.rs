//! Tool Gateway Router — in-process MCP server that aggregates tools.
//!
//! This module implements an MCP server that:
//! - Aggregates `tools/list` from all registered tools in `MetadataStore`.
//! - Dispatches `tools/call` to the appropriate tool backend.
//!
//! Runs as part of the gateway process, exposed at `POST /mcp`.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
};
use mcp_oxide_core::{Error, Result};
use mcp_oxide_mcp::jsonrpc::{ErrorObject, Request as JsonRequest, Response as JsonResponse};
use serde_json::json;
use tracing::debug;

use crate::{auth::AuthUser, error::AppError, state::AppState};

/// Handle `POST /mcp` — Tool Gateway Router.
pub async fn invoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    req: Request<Body>,
) -> Result<Response, AppError> {
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .map_err(|e| Error::Internal(format!("read body: {e}")))?;

    let rpc: JsonRequest = serde_json::from_slice(&body)
        .map_err(|e| Error::InvalidRequest(format!("parse json-rpc: {e}")))?;

    debug!(method = %rpc.method, id = ?rpc.id, "tool router request");

    let response = match rpc.method.as_str() {
        "tools/list" => handle_tools_list(&state, &rpc).await,
        "tools/call" => handle_tools_call(&state, &user, &rpc).await,
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
                code: -32601,
                message: "method not found".into(),
                data: None,
            }),
        }),
    };

    match response {
        Ok(resp) => {
            let body = serde_json::to_vec(&resp)
                .map_err(|e| Error::Internal(format!("serialize: {e}")))?;
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap())
        }
        Err(e) => {
            let resp = JsonResponse {
                jsonrpc: "2.0".into(),
                id: rpc.id,
                result: None,
                error: Some(ErrorObject {
                    code: -32603,
                    message: e.to_string(),
                    data: None,
                }),
            };
            let body = serde_json::to_vec(&resp)
                .map_err(|e| Error::Internal(format!("serialize: {e}")))?;
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap())
        }
    }
}

async fn handle_tools_list(state: &AppState, rpc: &JsonRequest) -> Result<JsonResponse> {
    let tools = state.metadata.list_tools(&mcp_oxide_core::providers::Filter::default()).await?;
    
    let tool_defs: Vec<serde_json::Value> = tools
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

    Ok(JsonResponse {
        jsonrpc: "2.0".into(),
        id: rpc.id.clone(),
        result: Some(json!({
            "tools": tool_defs
        })),
        error: None,
    })
}

async fn handle_tools_call(state: &AppState, _user: &mcp_oxide_core::identity::UserContext, rpc: &JsonRequest) -> Result<JsonResponse> {
    let params = rpc.params.clone().unwrap_or(json!({}));
    let tool_name = params["name"]
        .as_str()
        .ok_or_else(|| Error::InvalidRequest("missing tool name".into()))?
        .to_string();

    let _tool = state.metadata.get_tool(&tool_name).await?
        .ok_or_else(|| Error::NotFound(format!("tool '{tool_name}'")))?;

    // AuthZ check — TODO: call policy engine with action `tools/call` + tool metadata
    
    // Get the tool's endpoint from the deployment provider
    let handle = mcp_oxide_core::providers::DeploymentHandle {
        id: tool_name.clone(),
        namespace: None,
    };

    let endpoints = state.deployment.endpoints(&handle).await?;
    let endpoint = endpoints.into_iter().next()
        .ok_or_else(|| Error::Internal(format!("no endpoint for tool '{tool_name}'")))?;

    // Forward the tools/call to the tool backend
    let forward_req = JsonRequest {
        jsonrpc: "2.0".into(),
        id: rpc.id.clone(),
        method: "tools/call".into(),
        params: Some(params),
    };

    let resp = state.http
        .post(&endpoint.url)
        .json(&forward_req)
        .send()
        .await
        .map_err(|e| Error::UpstreamUnavailable(format!("tool backend: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(Error::UpstreamUnavailable(format!("tool backend returned {status}")));
    }

    let result: serde_json::Value = resp.json().await
        .map_err(|e| Error::Internal(format!("parse response: {e}")))?;

    Ok(JsonResponse {
        jsonrpc: "2.0".into(),
        id: rpc.id.clone(),
        result: Some(result),
        error: None,
    })
}
