//! Control-plane: CRUD endpoints for `/adapters` and `/tools`.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use mcp_oxide_core::{
    adapter::{Adapter, Endpoint, EnvVar, HealthProbe, ImageRef, Resources, SecretRef, SessionAffinity},
    audit::AuditDecision,
    providers::Filter,
    tool::{Tool, ToolDefinition},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    auth::AuthUser,
    error::AppError,
    routes::control_plane_helpers::{
        authorize, emit_audit, etag_for, extract_trace_id, parse_if_match, CpKind, CpVerb, ETAG,
    },
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub tags: Option<String>,
}

fn parse_tags(tags: Option<String>) -> Vec<String> {
    tags.map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
pub struct CreateAdapterBody {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub image: String,
    #[serde(default = "default_port")]
    pub endpoint_port: u16,
    #[serde(default = "default_path")]
    pub endpoint_path: String,
    #[serde(default)]
    pub upstream: Option<String>,
    #[serde(default = "one")]
    pub replicas: u32,
    #[serde(default)]
    pub env: Vec<EnvVarBody>,
    #[serde(default)]
    pub secret_refs: Vec<SecretRefBody>,
    #[serde(default)]
    pub required_roles: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub resources: Option<ResourcesBody>,
    #[serde(default)]
    pub health: Option<HealthProbeBody>,
    #[serde(default)]
    pub session_affinity: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

fn default_port() -> u16 { 8080 }
fn default_path() -> String { "/mcp".into() }
fn one() -> u32 { 1 }

#[derive(Debug, Deserialize)]
pub struct EnvVarBody {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct SecretRefBody {
    pub name: String,
    pub provider: String,
    pub key: String,
}

#[derive(Debug, Deserialize)]
pub struct ResourcesBody {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HealthProbeBody {
    pub path: String,
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAdapterBody {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub endpoint_port: Option<u16>,
    #[serde(default)]
    pub endpoint_path: Option<String>,
    #[serde(default)]
    pub upstream: Option<String>,
    #[serde(default)]
    pub replicas: Option<u32>,
    #[serde(default)]
    pub env: Option<Vec<EnvVarBody>>,
    #[serde(default)]
    pub secret_refs: Option<Vec<SecretRefBody>>,
    #[serde(default)]
    pub required_roles: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub resources: Option<ResourcesBody>,
    #[serde(default)]
    pub health: Option<HealthProbeBody>,
    #[serde(default)]
    pub session_affinity: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateToolBody {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub image: String,
    #[serde(default = "default_port")]
    pub endpoint_port: u16,
    #[serde(default = "default_path")]
    pub endpoint_path: String,
    pub tool_definition: ToolDefinitionBody,
    #[serde(default)]
    pub env: Vec<EnvVarBody>,
    #[serde(default)]
    pub secret_refs: Vec<SecretRefBody>,
    #[serde(default)]
    pub required_roles: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub resources: Option<ResourcesBody>,
}

#[derive(Debug, Deserialize)]
pub struct ToolDefinitionBody {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub annotations: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateToolBody {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub endpoint_port: Option<u16>,
    #[serde(default)]
    pub endpoint_path: Option<String>,
    #[serde(default)]
    pub tool_definition: Option<ToolDefinitionBody>,
    #[serde(default)]
    pub env: Option<Vec<EnvVarBody>>,
    #[serde(default)]
    pub secret_refs: Option<Vec<SecretRefBody>>,
    #[serde(default)]
    pub required_roles: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub resources: Option<ResourcesBody>,
    #[serde(default)]
    pub revision: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct AdapterResponse {
    pub name: String,
    pub description: Option<String>,
    pub image: String,
    pub endpoint_port: u16,
    pub endpoint_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub replicas: u32,
    pub env: Vec<EnvVarResponse>,
    pub secret_refs: Vec<SecretRefResponse>,
    pub required_roles: Vec<String>,
    pub tags: Vec<String>,
    pub resources: Option<ResourcesResponse>,
    pub health: Option<HealthProbeResponse>,
    pub session_affinity: String,
    pub labels: BTreeMap<String, String>,
    pub revision: u64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ToolResponse {
    pub name: String,
    pub description: Option<String>,
    pub image: String,
    pub endpoint_port: u16,
    pub endpoint_path: String,
    pub tool_definition: ToolDefinitionResponse,
    pub env: Vec<EnvVarResponse>,
    pub secret_refs: Vec<SecretRefResponse>,
    pub required_roles: Vec<String>,
    pub tags: Vec<String>,
    pub resources: Option<ResourcesResponse>,
    pub revision: u64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EnvVarResponse {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct SecretRefResponse {
    pub name: String,
    pub provider: String,
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct ResourcesResponse {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HealthProbeResponse {
    pub path: String,
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct ToolDefinitionResponse {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    pub annotations: Option<serde_json::Value>,
}

impl From<&Adapter> for AdapterResponse {
    fn from(a: &Adapter) -> Self {
        Self {
            name: a.name.clone(),
            description: a.description.clone(),
            image: a.image.reference.clone(),
            endpoint_port: a.endpoint.port,
            endpoint_path: a.endpoint.path.clone(),
            upstream: a.upstream.clone(),
            replicas: a.replicas,
            env: a.env.iter().map(|e| EnvVarResponse { name: e.name.clone(), value: e.value.clone() }).collect(),
            secret_refs: a.secret_refs.iter().map(|s| SecretRefResponse { name: s.name.clone(), provider: s.provider.clone(), key: s.key.clone() }).collect(),
            required_roles: a.required_roles.clone(),
            tags: a.tags.clone(),
            resources: a.resources.cpu.as_ref().or(a.resources.memory.as_ref()).map(|_| ResourcesResponse { cpu: a.resources.cpu.clone(), memory: a.resources.memory.clone() }),
            health: a.health.as_ref().map(|h| HealthProbeResponse { path: h.path.clone(), port: h.port }),
            session_affinity: match a.session_affinity {
                SessionAffinity::Sticky => "sticky",
                SessionAffinity::None => "none",
            }.to_string(),
            labels: a.labels.clone(),
            revision: a.revision.unwrap_or(0),
            created_at: a.created_at.map(|t| t.to_rfc3339()),
            updated_at: a.updated_at.map(|t| t.to_rfc3339()),
        }
    }
}

impl From<&Tool> for ToolResponse {
    fn from(t: &Tool) -> Self {
        Self {
            name: t.name.clone(),
            description: t.description.clone(),
            image: t.image.reference.clone(),
            endpoint_port: t.endpoint.port,
            endpoint_path: t.endpoint.path.clone(),
            tool_definition: ToolDefinitionResponse {
                name: t.tool_definition.name.clone(),
                title: t.tool_definition.title.clone(),
                description: t.tool_definition.description.clone(),
                input_schema: t.tool_definition.input_schema.clone(),
                annotations: t.tool_definition.annotations.clone(),
            },
            env: t.env.iter().map(|e| EnvVarResponse { name: e.name.clone(), value: e.value.clone() }).collect(),
            secret_refs: t.secret_refs.iter().map(|s| SecretRefResponse { name: s.name.clone(), provider: s.provider.clone(), key: s.key.clone() }).collect(),
            required_roles: t.required_roles.clone(),
            tags: t.tags.clone(),
            resources: t.resources.cpu.as_ref().or(t.resources.memory.as_ref()).map(|_| ResourcesResponse { cpu: t.resources.cpu.clone(), memory: t.resources.memory.clone() }),
            revision: t.revision.unwrap_or(0),
            created_at: t.created_at.map(|t| t.to_rfc3339()),
            updated_at: t.updated_at.map(|t| t.to_rfc3339()),
        }
    }
}

pub async fn list_adapters(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<AdapterResponse>>, AppError> {
    let trace_id = extract_trace_id(&headers);
    match authorize(&state, &user, CpKind::Adapter, CpVerb::List, "*", &[]).await {
        Ok(pid) => {
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::List, "*", AuditDecision::Allow, pid, None).await;
        }
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::List, "*", AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    }
    let filter = Filter { tenant: None, tags: parse_tags(query.tags) };
    let adapters = state.metadata.list_adapters(&filter).await?;
    Ok(Json(adapters.iter().map(AdapterResponse::from).collect()))
}

pub async fn create_adapter(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(body): Json<CreateAdapterBody>,
) -> Result<Response, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Adapter, CpVerb::Create, &body.name, &body.tags).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Create, &body.name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };

    let existing = state.metadata.get_adapter(&body.name).await?;
    if existing.is_some() {
        let err = AppError::Core(mcp_oxide_core::Error::Conflict(format!("adapter '{}' already exists", body.name)));
        emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Create, &body.name, AuditDecision::Deny, policy_id, Some(&err.to_string())).await;
        return Err(err);
    }

    let now = Utc::now();
    let adapter = Adapter {
        name: body.name.clone(),
        description: body.description,
        image: ImageRef { reference: body.image.clone() },
        endpoint: Endpoint { port: body.endpoint_port, path: body.endpoint_path.clone() },
        upstream: body.upstream.clone(),
        replicas: body.replicas,
        env: body.env.into_iter().map(|e| EnvVar { name: e.name, value: e.value }).collect(),
        secret_refs: body.secret_refs.into_iter().map(|s| SecretRef { name: s.name, provider: s.provider, key: s.key }).collect(),
        required_roles: body.required_roles,
        tags: body.tags,
        resources: Resources { cpu: body.resources.as_ref().and_then(|r| r.cpu.clone()), memory: body.resources.as_ref().and_then(|r| r.memory.clone()) },
        health: body.health.map(|h| HealthProbe { path: h.path, port: h.port }),
        session_affinity: match body.session_affinity.as_deref() {
            Some("none") => SessionAffinity::None,
            _ => SessionAffinity::Sticky,
        },
        labels: body.labels,
        revision: Some(1),
        created_at: Some(now),
        updated_at: Some(now),
    };

    // Deploy via DeploymentProvider if no explicit upstream URL.
    let _handle = if body.upstream.is_none() {
        let spec = mcp_oxide_core::providers::DeploymentSpec {
            name: body.name.clone(),
            kind: mcp_oxide_core::providers::DeploymentKind::Adapter,
            adapter: Some(adapter.clone()),
            tool: None,
        };
        state.deployment.apply(&spec).await?
    } else {
        mcp_oxide_core::providers::DeploymentHandle {
            id: body.name.clone(),
            namespace: None,
        }
    };

    state.metadata.put_adapter(&adapter).await?;
    emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Create, &body.name, AuditDecision::Allow, policy_id, None).await;

    let resp = AdapterResponse::from(&adapter);
    Ok((
        StatusCode::CREATED,
        [
            (header::LOCATION, format!("/adapters/{}", body.name).parse().unwrap()),
            (ETAG, etag_for(1)),
        ],
        Json(resp),
    ).into_response())
}

pub async fn get_adapter(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Adapter, CpVerb::Read, &name, &[]).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Read, &name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };
    let adapter = state.metadata.get_adapter(&name).await?
        .ok_or_else(|| mcp_oxide_core::Error::NotFound(format!("adapter '{name}'")))?;
    emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Read, &name, AuditDecision::Allow, policy_id, None).await;
    let rev = adapter.revision.unwrap_or(0);
    Ok((
        StatusCode::OK,
        [(ETAG, etag_for(rev))],
        Json(AdapterResponse::from(&adapter)),
    ).into_response())
}

pub async fn update_adapter(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<UpdateAdapterBody>,
) -> Result<Response, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Adapter, CpVerb::Update, &name, &[]).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Update, &name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };

    // If-Match header takes precedence over body.revision.
    let header_rev = parse_if_match(&headers)?;

    let mut adapter = state.metadata.get_adapter(&name).await?
        .ok_or_else(|| mcp_oxide_core::Error::NotFound(format!("adapter '{name}'")))?;

    let expected = header_rev.or(body.revision);
    if let Some(rev) = expected {
        if Some(rev) != adapter.revision {
            let err = AppError::Core(mcp_oxide_core::Error::Conflict("revision mismatch".into()));
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Update, &name, AuditDecision::Deny, policy_id, Some(&err.to_string())).await;
            return Err(err);
        }
    }

    let now = Utc::now();
    let prev_created = adapter.created_at;

    if let Some(d) = body.description { adapter.description = Some(d); }
    if let Some(img) = body.image { adapter.image = ImageRef { reference: img }; }
    if let Some(p) = body.endpoint_port { adapter.endpoint.port = p; }
    if let Some(p) = body.endpoint_path { adapter.endpoint.path = p; }
    if body.upstream.is_some() { adapter.upstream = body.upstream; }
    if let Some(r) = body.replicas { adapter.replicas = r; }
    if let Some(e) = body.env { adapter.env = e.into_iter().map(|e| EnvVar { name: e.name, value: e.value }).collect(); }
    if let Some(s) = body.secret_refs { adapter.secret_refs = s.into_iter().map(|s| SecretRef { name: s.name, provider: s.provider, key: s.key }).collect(); }
    if let Some(r) = body.required_roles { adapter.required_roles = r; }
    if let Some(t) = body.tags { adapter.tags = t; }
    if let Some(r) = body.resources { adapter.resources = Resources { cpu: r.cpu, memory: r.memory }; }
    if body.health.is_some() { adapter.health = body.health.map(|h| HealthProbe { path: h.path, port: h.port }); }
    if let Some(s) = body.session_affinity { adapter.session_affinity = match s.as_str() { "none" => SessionAffinity::None, _ => SessionAffinity::Sticky }; }
    if let Some(l) = body.labels { adapter.labels = l; }

    let new_rev = adapter.revision.unwrap_or(0) + 1;
    adapter.revision = Some(new_rev);
    adapter.updated_at = Some(now);
    adapter.created_at = prev_created;

    state.metadata.put_adapter(&adapter).await?;
    emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Update, &name, AuditDecision::Allow, policy_id, None).await;
    Ok((
        StatusCode::OK,
        [(ETAG, etag_for(new_rev))],
        Json(AdapterResponse::from(&adapter)),
    ).into_response())
}

pub async fn delete_adapter(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Adapter, CpVerb::Delete, &name, &[]).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Delete, &name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };

    let existing = state.metadata.get_adapter(&name).await?;
    let Some(existing) = existing else {
        let err = AppError::Core(mcp_oxide_core::Error::NotFound(format!("adapter '{name}'")));
        emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Delete, &name, AuditDecision::Deny, policy_id, Some(&err.to_string())).await;
        return Err(err);
    };

    if let Some(rev) = parse_if_match(&headers)? {
        if Some(rev) != existing.revision {
            let err = AppError::Core(mcp_oxide_core::Error::Conflict("revision mismatch".into()));
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Delete, &name, AuditDecision::Deny, policy_id, Some(&err.to_string())).await;
            return Err(err);
        }
    }

    // Delete from deployment provider if no explicit upstream.
    if existing.upstream.is_none() {
        let handle = mcp_oxide_core::providers::DeploymentHandle {
            id: name.clone(),
            namespace: None,
        };
        let _ = state.deployment.delete(&handle).await;
    }

    state.metadata.delete_adapter(&name).await?;
    emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Delete, &name, AuditDecision::Allow, policy_id, None).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_tools(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<ToolResponse>>, AppError> {
    let trace_id = extract_trace_id(&headers);
    match authorize(&state, &user, CpKind::Tool, CpVerb::List, "*", &[]).await {
        Ok(pid) => {
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::List, "*", AuditDecision::Allow, pid, None).await;
        }
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::List, "*", AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    }
    let filter = Filter { tenant: None, tags: parse_tags(query.tags) };
    let tools = state.metadata.list_tools(&filter).await?;
    Ok(Json(tools.iter().map(ToolResponse::from).collect()))
}

pub async fn create_tool(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Json(body): Json<CreateToolBody>,
) -> Result<Response, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Tool, CpVerb::Create, &body.name, &body.tags).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Create, &body.name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };

    let existing = state.metadata.get_tool(&body.name).await?;
    if existing.is_some() {
        let err = AppError::Core(mcp_oxide_core::Error::Conflict(format!("tool '{}' already exists", body.name)));
        emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Create, &body.name, AuditDecision::Deny, policy_id, Some(&err.to_string())).await;
        return Err(err);
    }

    let now = Utc::now();
    let tool = Tool {
        name: body.name.clone(),
        description: body.description,
        image: ImageRef { reference: body.image },
        endpoint: Endpoint { port: body.endpoint_port, path: body.endpoint_path },
        tool_definition: ToolDefinition {
            name: body.tool_definition.name,
            title: body.tool_definition.title,
            description: body.tool_definition.description,
            input_schema: body.tool_definition.input_schema,
            annotations: body.tool_definition.annotations,
        },
        env: body.env.into_iter().map(|e| EnvVar { name: e.name, value: e.value }).collect(),
        secret_refs: body.secret_refs.into_iter().map(|s| SecretRef { name: s.name, provider: s.provider, key: s.key }).collect(),
        required_roles: body.required_roles,
        tags: body.tags,
        resources: Resources { cpu: body.resources.as_ref().and_then(|r| r.cpu.clone()), memory: body.resources.as_ref().and_then(|r| r.memory.clone()) },
        revision: Some(1),
        created_at: Some(now),
        updated_at: Some(now),
    };

    // Deploy via DeploymentProvider.
    let spec = mcp_oxide_core::providers::DeploymentSpec {
        name: body.name.clone(),
        kind: mcp_oxide_core::providers::DeploymentKind::Tool,
        adapter: None,
        tool: Some(tool.clone()),
    };
    let _handle = state.deployment.apply(&spec).await?;

    state.metadata.put_tool(&tool).await?;
    emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Create, &body.name, AuditDecision::Allow, policy_id, None).await;

    let resp = ToolResponse::from(&tool);
    Ok((
        StatusCode::CREATED,
        [
            (header::LOCATION, format!("/tools/{}", body.name).parse().unwrap()),
            (ETAG, etag_for(1)),
        ],
        Json(resp),
    ).into_response())
}

pub async fn get_tool(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Tool, CpVerb::Read, &name, &[]).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Read, &name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };
    let tool = state.metadata.get_tool(&name).await?
        .ok_or_else(|| mcp_oxide_core::Error::NotFound(format!("tool '{name}'")))?;
    emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Read, &name, AuditDecision::Allow, policy_id, None).await;
    let rev = tool.revision.unwrap_or(0);
    Ok((
        StatusCode::OK,
        [(ETAG, etag_for(rev))],
        Json(ToolResponse::from(&tool)),
    ).into_response())
}

pub async fn update_tool(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<UpdateToolBody>,
) -> Result<Response, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Tool, CpVerb::Update, &name, &[]).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Update, &name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };

    let header_rev = parse_if_match(&headers)?;

    let mut tool = state.metadata.get_tool(&name).await?
        .ok_or_else(|| mcp_oxide_core::Error::NotFound(format!("tool '{name}'")))?;

    let expected = header_rev.or(body.revision);
    if let Some(rev) = expected {
        if Some(rev) != tool.revision {
            let err = AppError::Core(mcp_oxide_core::Error::Conflict("revision mismatch".into()));
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Update, &name, AuditDecision::Deny, policy_id, Some(&err.to_string())).await;
            return Err(err);
        }
    }

    let now = Utc::now();
    let prev_created = tool.created_at;

    if let Some(d) = body.description { tool.description = Some(d); }
    if let Some(img) = body.image { tool.image = ImageRef { reference: img }; }
    if let Some(p) = body.endpoint_port { tool.endpoint.port = p; }
    if let Some(p) = body.endpoint_path { tool.endpoint.path = p; }
    if let Some(td) = body.tool_definition {
        tool.tool_definition = ToolDefinition {
            name: td.name,
            title: td.title,
            description: td.description,
            input_schema: td.input_schema,
            annotations: td.annotations,
        };
    }
    if let Some(e) = body.env { tool.env = e.into_iter().map(|e| EnvVar { name: e.name, value: e.value }).collect(); }
    if let Some(s) = body.secret_refs { tool.secret_refs = s.into_iter().map(|s| SecretRef { name: s.name, provider: s.provider, key: s.key }).collect(); }
    if let Some(r) = body.required_roles { tool.required_roles = r; }
    if let Some(t) = body.tags { tool.tags = t; }
    if let Some(r) = body.resources { tool.resources = Resources { cpu: r.cpu, memory: r.memory }; }

    let new_rev = tool.revision.unwrap_or(0) + 1;
    tool.revision = Some(new_rev);
    tool.updated_at = Some(now);
    tool.created_at = prev_created;

    state.metadata.put_tool(&tool).await?;
    emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Update, &name, AuditDecision::Allow, policy_id, None).await;
    Ok((
        StatusCode::OK,
        [(ETAG, etag_for(new_rev))],
        Json(ToolResponse::from(&tool)),
    ).into_response())
}

pub async fn delete_tool(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Tool, CpVerb::Delete, &name, &[]).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Delete, &name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };

    let existing = state.metadata.get_tool(&name).await?;
    let Some(existing) = existing else {
        let err = AppError::Core(mcp_oxide_core::Error::NotFound(format!("tool '{name}'")));
        emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Delete, &name, AuditDecision::Deny, policy_id, Some(&err.to_string())).await;
        return Err(err);
    };

    if let Some(rev) = parse_if_match(&headers)? {
        if Some(rev) != existing.revision {
            let err = AppError::Core(mcp_oxide_core::Error::Conflict("revision mismatch".into()));
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Delete, &name, AuditDecision::Deny, policy_id, Some(&err.to_string())).await;
            return Err(err);
        }
    }

    // Delete from deployment provider.
    let handle = mcp_oxide_core::providers::DeploymentHandle {
        id: name.clone(),
        namespace: None,
    };
    let _ = state.deployment.delete(&handle).await;

    state.metadata.delete_tool(&name).await?;
    emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Delete, &name, AuditDecision::Allow, policy_id, None).await;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Deployment status and logs endpoints
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DeploymentStatusResponse {
    pub ready: bool,
    pub replicas: u32,
    pub ready_replicas: u32,
    pub message: Option<String>,
}

pub async fn get_adapter_status(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<DeploymentStatusResponse>, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Adapter, CpVerb::Read, &name, &[]).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Read, &name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };

    let _adapter = state.metadata.get_adapter(&name).await?
        .ok_or_else(|| mcp_oxide_core::Error::NotFound(format!("adapter '{name}'")))?;

    let handle = mcp_oxide_core::providers::DeploymentHandle {
        id: name.clone(),
        namespace: None,
    };
    let _ = state.deployment.status(&handle).await;

    let status = state.deployment.status(&handle).await?;
    emit_audit(&state, &trace_id, &user, CpKind::Adapter, CpVerb::Read, &name, AuditDecision::Allow, policy_id, None).await;

    Ok(Json(DeploymentStatusResponse {
        ready: status.ready,
        replicas: status.replicas,
        ready_replicas: status.ready_replicas,
        message: status.message,
    }))
}

pub async fn get_tool_status(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<DeploymentStatusResponse>, AppError> {
    let trace_id = extract_trace_id(&headers);
    let policy_id = match authorize(&state, &user, CpKind::Tool, CpVerb::Read, &name, &[]).await {
        Ok(pid) => pid,
        Err(e) => {
            emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Read, &name, AuditDecision::Deny, None, Some(&e.to_string())).await;
            return Err(e);
        }
    };

    let _tool = state.metadata.get_tool(&name).await?
        .ok_or_else(|| mcp_oxide_core::Error::NotFound(format!("tool '{name}'")))?;

    let handle = mcp_oxide_core::providers::DeploymentHandle {
        id: name.clone(),
        namespace: None,
    };
    let _ = state.deployment.status(&handle).await;

    let status = state.deployment.status(&handle).await?;
    emit_audit(&state, &trace_id, &user, CpKind::Tool, CpVerb::Read, &name, AuditDecision::Allow, policy_id, None).await;

    Ok(Json(DeploymentStatusResponse {
        ready: status.ready,
        replicas: status.replicas,
        ready_replicas: status.ready_replicas,
        message: status.message,
    }))
}
