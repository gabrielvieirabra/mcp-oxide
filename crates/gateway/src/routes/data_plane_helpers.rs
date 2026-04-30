//! Data-plane helpers: authz enforcement, audit emission, name validation.
//!
//! Shared between `/adapters/{name}/mcp` and `/mcp` (tool router) so that
//! every data-plane invocation goes through the same authz + audit pipeline.

use axum::http::HeaderMap;
use mcp_oxide_core::{
    audit::{AuditDecision, AuditRecord, AuditTarget, AuditUser},
    identity::UserContext,
    policy::{Action, Env, Plane, PolicyInput, Resource, ResourceKind},
};

use crate::state::AppState;

/// Extract a trace id from `x-request-id` or generate one.
pub fn extract_trace_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), ToOwned::to_owned)
}

/// Outcome of a data-plane authz check.
#[derive(Debug)]
pub struct DataPlaneAuthz {
    pub policy_id: Option<String>,
}

/// Authorize a data-plane operation against `target`, enforcing both the
/// policy engine decision AND the resource's `required_roles` (defense in
/// depth). Returns `Err(reason)` when denied.
#[allow(clippy::too_many_arguments)]
pub async fn authorize_data_plane(
    state: &AppState,
    user: &UserContext,
    method: &str,
    tool: Option<&str>,
    target_kind: ResourceKind,
    target_name: &str,
    target_tags: &[String],
    required_roles: &[String],
) -> Result<DataPlaneAuthz, String> {
    let input = PolicyInput {
        user,
        action: Action {
            plane: Plane::Data,
            method,
            tool,
        },
        resource: Resource {
            kind: target_kind,
            name: target_name,
            tags: target_tags.to_vec(),
            required_roles: required_roles.to_vec(),
        },
        env: Env::default(),
    };
    let decision = state
        .authz
        .decide(&input)
        .await
        .map_err(|e| e.to_string())?;
    if !decision.allow {
        return Err(decision.reason.unwrap_or_else(|| "forbidden".into()));
    }
    if !required_roles_satisfied(required_roles, &user.roles) {
        return Err("required_roles".into());
    }
    Ok(DataPlaneAuthz {
        policy_id: decision.policy_id,
    })
}

/// Check whether the user satisfies any of the `required_roles` (empty list
/// means "no restriction").
#[must_use]
pub fn required_roles_satisfied(required: &[String], user_roles: &[String]) -> bool {
    if required.is_empty() {
        return true;
    }
    required.iter().any(|r| user_roles.iter().any(|u| u == r))
}

/// Emit a data-plane audit record. Best-effort — logs at warn on failure.
#[allow(clippy::too_many_arguments)]
pub async fn emit_audit(
    state: &AppState,
    trace_id: &str,
    user: &UserContext,
    action: &str,
    target_kind: &str,
    target_name: &str,
    decision: AuditDecision,
    policy_id: Option<String>,
    latency_ms: u64,
    upstream_status: &str,
    request_hash: &str,
    err: Option<&str>,
) {
    let record = AuditRecord {
        ts: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        trace_id: trace_id.into(),
        user: AuditUser::from(user),
        plane: Plane::Data,
        action: action.into(),
        target: AuditTarget {
            kind: target_kind.into(),
            name: target_name.into(),
        },
        decision,
        policy_id,
        latency_ms,
        upstream_status: upstream_status.into(),
        request_hash: request_hash.into(),
        error: err.map(ToOwned::to_owned),
    };
    if let Err(e) = state.audit.emit(&record).await {
        tracing::warn!(error=%e, "audit emit failed");
    }
}
