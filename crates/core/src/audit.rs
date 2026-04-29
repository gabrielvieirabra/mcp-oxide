//! Audit record schema.

use crate::identity::UserContext;
use crate::policy::Plane;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub ts: String,
    pub trace_id: String,
    pub user: AuditUser,
    pub plane: Plane,
    pub action: String,
    pub target: AuditTarget,
    pub decision: AuditDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    pub latency_ms: u64,
    pub upstream_status: String,
    pub request_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditUser {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

impl From<&UserContext> for AuditUser {
    fn from(u: &UserContext) -> Self {
        Self {
            sub: u.sub.clone(),
            tenant: u.tenant.clone(),
            roles: u.roles.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditTarget {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuditDecision {
    Allow,
    Deny,
}

impl Plane {
    pub fn as_str(self) -> &'static str {
        match self {
            Plane::Control => "control",
            Plane::Data => "data",
        }
    }
}
