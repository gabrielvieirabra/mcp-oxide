//! Data-plane: `/adapters/{name}/mcp` JSON-RPC (and SSE) reverse proxy.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use mcp_oxide_core::{
    audit::{AuditDecision, AuditRecord, AuditTarget, AuditUser},
    policy::{Action, Env, Plane, PolicyInput, Resource, ResourceKind},
};
use serde_json::json;

use crate::{
    auth::AuthUser,
    proxy::{self, ProxyError},
    state::AppState,
};

#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
pub async fn invoke(
    State(state): State<AppState>,
    Path(name): Path<String>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let trace_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), ToOwned::to_owned);

    // Resolve adapter.
    let Some(adapter) = state.adapters.get(&name).cloned() else {
        emit_audit(
            &state,
            &trace_id,
            &user,
            "mcp.invoke",
            ("adapter", name.as_str()),
            AuditDecision::Deny,
            None,
            0,
            "not_found",
            Some("adapter not found"),
        )
        .await;
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not_found", "message": "adapter not found" })),
        )
            .into_response();
    };

    // Extract JSON-RPC method for policy evaluation (best-effort — invalid
    // JSON flows through as "unknown" and is likely denied).
    let method = parse_jsonrpc_method(&body).unwrap_or_else(|| "unknown".into());

    // Authorize.
    let input = PolicyInput {
        user: &user,
        action: Action {
            plane: Plane::Data,
            method: &method,
            tool: None,
        },
        resource: Resource {
            kind: ResourceKind::Adapter,
            name: &adapter.name,
            tags: adapter.tags.clone(),
            required_roles: adapter.required_roles.clone(),
        },
        env: Env::default(),
    };
    let decision = match state.authz.decide(&input).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error=%e, "policy error");
            emit_audit(
                &state,
                &trace_id,
                &user,
                &method,
                ("adapter", &adapter.name),
                AuditDecision::Deny,
                None,
                0,
                "policy_error",
                Some(&e.to_string()),
            )
            .await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal_error", "message": "policy" })),
            )
                .into_response();
        }
    };

    if !decision.allow {
        let reason = decision.reason.clone().unwrap_or_default();
        emit_audit(
            &state,
            &trace_id,
            &user,
            &method,
            ("adapter", &adapter.name),
            AuditDecision::Deny,
            decision.policy_id.clone(),
            0,
            "forbidden",
            Some(&reason),
        )
        .await;
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": reason })),
        )
            .into_response();
    }

    // Also enforce required_roles on the adapter itself (defense in depth).
    if !required_roles_satisfied(&adapter.required_roles, &user.roles) {
        emit_audit(
            &state,
            &trace_id,
            &user,
            &method,
            ("adapter", &adapter.name),
            AuditDecision::Deny,
            decision.policy_id.clone(),
            0,
            "forbidden",
            Some("required_roles"),
        )
        .await;
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "forbidden", "message": "required_roles" })),
        )
            .into_response();
    }

    // Forward.
    let forward_headers = proxy::forwardable_headers(&headers);
    let request_hash = format!("sha256:{}", proxy::sha256_hex(&body));

    let (resp, outcome_kind, outcome_msg, status_label, latency_ms) =
        match proxy::forward_post(&state.http, &adapter.upstream, forward_headers, body).await {
            Ok((r, outcome)) => {
                let latency = outcome.latency.as_millis() as u64;
                let label = if outcome.status.is_success() {
                    "ok"
                } else if outcome.status.is_server_error() {
                    "upstream_error"
                } else {
                    "upstream_client_error"
                };
                (r, AuditDecision::Allow, None, label.to_string(), latency)
            }
            Err(ProxyError::Timeout(msg)) => (
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(json!({ "error": "upstream_timeout", "message": msg.clone() })),
                )
                    .into_response(),
                AuditDecision::Allow,
                Some(msg),
                "upstream_timeout".into(),
                0,
            ),
            Err(ProxyError::Unavailable(msg)) => (
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": "upstream_unavailable", "message": msg.clone() })),
                )
                    .into_response(),
                AuditDecision::Allow,
                Some(msg),
                "upstream_unavailable".into(),
                0,
            ),
        };

    // Emit audit (post-forward). request_hash kept to support replay detection.
    let record = AuditRecord {
        ts: current_rfc3339(),
        trace_id: trace_id.clone(),
        user: AuditUser::from(&user),
        plane: Plane::Data,
        action: method,
        target: AuditTarget {
            kind: "adapter".into(),
            name: adapter.name.clone(),
        },
        decision: outcome_kind,
        policy_id: decision.policy_id.clone(),
        latency_ms,
        upstream_status: status_label,
        request_hash,
        error: outcome_msg,
    };
    if let Err(e) = state.audit.emit(&record).await {
        tracing::warn!(error=%e, "audit emit failed");
    }

    resp
}

fn parse_jsonrpc_method(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("method")
        .and_then(|m| m.as_str())
        .map(ToOwned::to_owned)
}

fn required_roles_satisfied(required: &[String], user_roles: &[String]) -> bool {
    if required.is_empty() {
        return true;
    }
    required.iter().any(|r| user_roles.iter().any(|u| u == r))
}

fn current_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
async fn emit_audit(
    state: &AppState,
    trace_id: &str,
    user: &mcp_oxide_core::identity::UserContext,
    action: &str,
    target: (&str, &str),
    decision: AuditDecision,
    policy_id: Option<String>,
    latency_ms: u64,
    upstream_status: &str,
    err: Option<&str>,
) {
    let record = AuditRecord {
        ts: current_rfc3339(),
        trace_id: trace_id.into(),
        user: AuditUser::from(user),
        plane: Plane::Data,
        action: action.into(),
        target: AuditTarget {
            kind: target.0.into(),
            name: target.1.into(),
        },
        decision,
        policy_id,
        latency_ms,
        upstream_status: upstream_status.into(),
        request_hash: String::new(),
        error: err.map(ToOwned::to_owned),
    };
    if let Err(e) = state.audit.emit(&record).await {
        tracing::warn!(error=%e, "audit emit failed");
    }
}
