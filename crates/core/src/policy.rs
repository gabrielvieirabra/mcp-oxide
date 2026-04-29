//! Policy engine input/decision contracts.

use crate::identity::UserContext;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct PolicyInput<'a> {
    pub user: &'a UserContext,
    pub action: Action<'a>,
    pub resource: Resource<'a>,
    pub env: Env<'a>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Plane {
    Control,
    Data,
}

#[derive(Debug, Clone, Serialize)]
pub struct Action<'a> {
    pub plane: Plane,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Resource<'a> {
    pub kind: ResourceKind,
    pub name: &'a str,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_roles: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    Adapter,
    Tool,
    Gateway,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Env<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Decision {
    pub allow: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
}

impl Decision {
    pub fn allow() -> Self {
        Self {
            allow: true,
            reason: None,
            policy_id: None,
        }
    }
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            allow: false,
            reason: Some(reason.into()),
            policy_id: None,
        }
    }
}
