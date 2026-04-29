//! YAML RBAC policy engine.
//!
//! Policy file schema:
//!
//! ```yaml
//! version: 1
//! default: deny        # deny | allow
//! rules:
//!   - plane: data      # control | data (optional)
//!     action: "tools/call"
//!     target: "weather"           # exact target name match (optional)
//!     target_tags: ["public"]     # all listed tags must be present (optional)
//!     allow_roles: ["mcp.engineer", "*"]   # "*" means any authenticated user
//! ```
//!
//! Action matching uses simple glob-style prefix matching with `*` as a
//! terminal wildcard (e.g. `adapters.*` matches `adapters.read`).

use std::path::Path;

use async_trait::async_trait;
use mcp_oxide_core::{
    policy::{Decision, Plane, PolicyInput},
    providers::PolicyEngine,
    Error, Result,
};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct YamlRbacPolicy {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_default")]
    pub default: DefaultDecision,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

fn default_version() -> u32 {
    1
}
fn default_default() -> DefaultDecision {
    DefaultDecision::Deny
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DefaultDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub plane: Option<Plane>,
    pub action: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub target_tags: Vec<String>,
    #[serde(default)]
    pub allow_roles: Vec<String>,
}

#[derive(Debug)]
pub struct YamlRbacEngine {
    policy: YamlRbacPolicy,
    source: String,
}

impl YamlRbacEngine {
    pub fn from_str(yaml: &str, source: impl Into<String>) -> Result<Self> {
        let policy: YamlRbacPolicy =
            serde_yaml::from_str(yaml).map_err(|e| Error::Internal(format!("policy yaml: {e}")))?;
        if policy.version != 1 {
            return Err(Error::Internal(format!(
                "policy version {} unsupported (expected 1)",
                policy.version
            )));
        }
        Ok(Self {
            policy,
            source: source.into(),
        })
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref();
        let s = std::fs::read_to_string(p)
            .map_err(|e| Error::Internal(format!("policy read {}: {e}", p.display())))?;
        Self::from_str(&s, p.display().to_string())
    }

    #[must_use]
    pub fn policy(&self) -> &YamlRbacPolicy {
        &self.policy
    }
}

#[async_trait]
impl PolicyEngine for YamlRbacEngine {
    async fn decide(&self, input: &PolicyInput<'_>) -> Result<Decision> {
        for rule in &self.policy.rules {
            if rule_matches(rule, input) && roles_allowed(rule, input) {
                return Ok(Decision {
                    allow: true,
                    reason: Some(format!("rule:{}", rule.action)),
                    policy_id: Some(format!("yaml-rbac:{}", self.source)),
                });
            }
        }
        Ok(match self.policy.default {
            DefaultDecision::Allow => Decision {
                allow: true,
                reason: Some("default-allow".into()),
                policy_id: Some(format!("yaml-rbac:{}", self.source)),
            },
            DefaultDecision::Deny => Decision {
                allow: false,
                reason: Some("default-deny".into()),
                policy_id: Some(format!("yaml-rbac:{}", self.source)),
            },
        })
    }

    fn kind(&self) -> &'static str {
        "yaml-rbac"
    }
}

fn rule_matches(rule: &Rule, input: &PolicyInput<'_>) -> bool {
    if let Some(p) = rule.plane {
        if p as u8 != input.action.plane as u8 {
            return false;
        }
    }
    if !action_glob_matches(&rule.action, input.action.method) {
        return false;
    }
    if let Some(t) = rule.target.as_deref() {
        if t != input.resource.name {
            return false;
        }
    }
    if !rule.target_tags.is_empty() {
        for tag in &rule.target_tags {
            if !input.resource.tags.iter().any(|r| r == tag) {
                return false;
            }
        }
    }
    true
}

fn action_glob_matches(pattern: &str, action: &str) -> bool {
    if pattern == "*" || pattern == action {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return action == prefix || action.starts_with(&format!("{prefix}."));
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return action.starts_with(prefix);
    }
    false
}

fn roles_allowed(rule: &Rule, input: &PolicyInput<'_>) -> bool {
    if rule.allow_roles.is_empty() {
        return false;
    }
    for r in &rule.allow_roles {
        if r == "*" {
            return true;
        }
        if input.user.roles.iter().any(|u| u == r) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_oxide_core::{
        identity::UserContext,
        policy::{Action, Env, Resource, ResourceKind},
    };

    fn u(roles: &[&str]) -> UserContext {
        UserContext {
            sub: "alice".into(),
            roles: roles.iter().map(|s| (*s).into()).collect(),
            ..Default::default()
        }
    }

    fn input<'a>(
        user: &'a UserContext,
        method: &'a str,
        target: &'a str,
        tags: Vec<String>,
    ) -> PolicyInput<'a> {
        PolicyInput {
            user,
            action: Action {
                plane: Plane::Data,
                method,
                tool: None,
            },
            resource: Resource {
                kind: ResourceKind::Tool,
                name: target,
                tags,
                required_roles: vec![],
            },
            env: Env::default(),
        }
    }

    #[tokio::test]
    async fn rbac_allow_and_default_deny() {
        let yaml = r#"
version: 1
default: deny
rules:
  - plane: data
    action: "tools/list"
    allow_roles: ["*"]
  - plane: data
    action: "tools/call"
    target: "weather"
    allow_roles: ["mcp.engineer"]
  - plane: data
    action: "tools/call"
    target_tags: ["mutating"]
    allow_roles: ["mcp.admin"]
        "#;
        let e = YamlRbacEngine::from_str(yaml, "t").unwrap();
        let user = u(&["mcp.engineer"]);

        let d = e
            .decide(&input(&user, "tools/list", "*", vec![]))
            .await
            .unwrap();
        assert!(d.allow);

        let d = e
            .decide(&input(&user, "tools/call", "weather", vec![]))
            .await
            .unwrap();
        assert!(d.allow);

        let d = e
            .decide(&input(&user, "tools/call", "other", vec![]))
            .await
            .unwrap();
        assert!(!d.allow);

        // Mutating needs admin.
        let d = e
            .decide(&input(
                &user,
                "tools/call",
                "deleter",
                vec!["mutating".into()],
            ))
            .await
            .unwrap();
        assert!(!d.allow);

        let admin = u(&["mcp.admin"]);
        let d = e
            .decide(&input(
                &admin,
                "tools/call",
                "deleter",
                vec!["mutating".into()],
            ))
            .await
            .unwrap();
        assert!(d.allow);
    }

    #[test]
    fn action_globs() {
        assert!(action_glob_matches("*", "anything"));
        assert!(action_glob_matches("adapters.*", "adapters.read"));
        assert!(action_glob_matches("adapters.*", "adapters"));
        assert!(!action_glob_matches("adapters.*", "tools"));
        assert!(action_glob_matches("tools/", "tools/"));
        assert!(action_glob_matches("tools/*", "tools/call"));
    }
}
