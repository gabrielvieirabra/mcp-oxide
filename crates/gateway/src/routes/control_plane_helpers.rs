//! Control-plane helpers: authz enforcement, audit emission, `ETag` handling.

use axum::http::{header::HeaderName, HeaderMap, HeaderValue};
use mcp_oxide_core::{
    audit::{AuditDecision, AuditRecord, AuditTarget, AuditUser},
    identity::UserContext,
    policy::{Action, Env, Plane, PolicyInput, Resource, ResourceKind},
};

use crate::{error::AppError, state::AppState};

pub const IF_MATCH: HeaderName = HeaderName::from_static("if-match");
pub const ETAG: HeaderName = HeaderName::from_static("etag");

/// Control-plane resource kind ("adapter" | "tool").
#[derive(Debug, Clone, Copy)]
pub enum CpKind {
    Adapter,
    Tool,
}

impl CpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CpKind::Adapter => "adapter",
            CpKind::Tool => "tool",
        }
    }

    pub fn resource_kind(self) -> ResourceKind {
        match self {
            CpKind::Adapter => ResourceKind::Adapter,
            CpKind::Tool => ResourceKind::Tool,
        }
    }

    /// Action namespace used in RBAC rules ("adapters" | "tools").
    pub fn action_ns(self) -> &'static str {
        match self {
            CpKind::Adapter => "adapters",
            CpKind::Tool => "tools",
        }
    }
}

/// Control-plane verb.
#[derive(Debug, Clone, Copy)]
pub enum CpVerb {
    Create,
    Read,
    List,
    Update,
    Delete,
}

impl CpVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            CpVerb::Create => "create",
            CpVerb::Read => "read",
            CpVerb::List => "list",
            CpVerb::Update => "update",
            CpVerb::Delete => "delete",
        }
    }
}

/// Authorize a control-plane operation. Returns the matched `policy_id` (for
/// audit). Does NOT emit audit events; the caller emits one per operation.
pub async fn authorize(
    state: &AppState,
    user: &UserContext,
    kind: CpKind,
    verb: CpVerb,
    name: &str,
    tags: &[String],
) -> Result<Option<String>, AppError> {
    let action = format!("{}.{}", kind.action_ns(), verb.as_str());
    let input = PolicyInput {
        user,
        action: Action {
            plane: Plane::Control,
            method: &action,
            tool: None,
        },
        resource: Resource {
            kind: kind.resource_kind(),
            name,
            tags: tags.to_vec(),
            required_roles: vec![],
        },
        env: Env::default(),
    };
    let decision = state.authz.decide(&input).await.map_err(AppError::Core)?;
    if !decision.allow {
        let reason = decision.reason.unwrap_or_else(|| "forbidden".into());
        return Err(AppError::Core(mcp_oxide_core::Error::Forbidden(reason)));
    }
    Ok(decision.policy_id)
}

/// Emit an audit record for a control-plane operation.
#[allow(clippy::too_many_arguments)]
pub async fn emit_audit(
    state: &AppState,
    trace_id: &str,
    user: &UserContext,
    kind: CpKind,
    verb: CpVerb,
    name: &str,
    decision: AuditDecision,
    policy_id: Option<String>,
    error: Option<&str>,
) {
    let record = AuditRecord {
        ts: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        trace_id: trace_id.into(),
        user: AuditUser::from(user),
        plane: Plane::Control,
        action: format!("{}.{}", kind.action_ns(), verb.as_str()),
        target: AuditTarget {
            kind: kind.as_str().into(),
            name: name.into(),
        },
        decision,
        policy_id,
        latency_ms: 0,
        upstream_status: String::new(),
        request_hash: String::new(),
        error: error.map(ToOwned::to_owned),
    };
    if let Err(e) = state.audit.emit(&record).await {
        tracing::warn!(error=%e, "audit emit failed");
    }
}

/// Extract trace id from x-request-id header or generate one.
pub fn extract_trace_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), ToOwned::to_owned)
}

/// Parse `If-Match` header as `W/"<revision>"` or `"<revision>"`. Returns
/// `None` if header absent; errors on malformed value.
pub fn parse_if_match(headers: &HeaderMap) -> Result<Option<u64>, AppError> {
    let Some(v) = headers.get(IF_MATCH) else {
        return Ok(None);
    };
    let s = v
        .to_str()
        .map_err(|_| AppError::Core(mcp_oxide_core::Error::InvalidRequest("if-match".into())))?;
    let s = s.trim();
    let s = s.strip_prefix("W/").unwrap_or(s);
    let s = s.trim_matches('"');
    s.parse::<u64>()
        .map(Some)
        .map_err(|_| AppError::Core(mcp_oxide_core::Error::InvalidRequest("if-match value".into())))
}

/// Build a weak `ETag` header value from a revision.
pub fn etag_for(revision: u64) -> HeaderValue {
    // Weak ETag because payload may have insignificant ordering differences.
    HeaderValue::from_str(&format!("W/\"{revision}\"")).expect("valid etag")
}
