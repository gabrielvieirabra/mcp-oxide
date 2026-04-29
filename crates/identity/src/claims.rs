//! Claim extraction helpers shared by identity providers.

use mcp_oxide_core::identity::UserContext;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ClaimExtractor {
    /// Dotted-path claim paths that may contain a `Vec<String>` of roles.
    pub role_paths: Vec<String>,
    /// Dotted-path claim paths that may contain a `Vec<String>` of groups.
    pub group_paths: Vec<String>,
    /// Dotted-path claim for tenant/org.
    pub tenant_path: Option<String>,
    /// Claim key for scopes (default `scope`, space-separated).
    pub scopes_path: String,
}

impl Default for ClaimExtractor {
    fn default() -> Self {
        Self {
            role_paths: vec!["realm_access.roles".to_string(), "roles".to_string()],
            group_paths: vec!["groups".to_string()],
            tenant_path: Some("tenant".to_string()),
            scopes_path: "scope".to_string(),
        }
    }
}

impl ClaimExtractor {
    pub fn extract(&self, claims: &Value) -> UserContext {
        let sub = get_string(claims, "sub").unwrap_or_default();

        let mut roles: Vec<String> = Vec::new();
        for path in &self.role_paths {
            roles.extend(get_string_vec(claims, path));
        }
        roles.sort();
        roles.dedup();

        let mut groups: Vec<String> = Vec::new();
        for path in &self.group_paths {
            groups.extend(get_string_vec(claims, path));
        }
        groups.sort();
        groups.dedup();

        let tenant = self
            .tenant_path
            .as_deref()
            .and_then(|p| get_string(claims, p));

        let scopes = get_string(claims, &self.scopes_path)
            .map(|s| s.split_whitespace().map(ToString::to_string).collect())
            .unwrap_or_default();

        UserContext {
            sub,
            tenant,
            roles,
            groups,
            scopes,
            claims: claims.clone(),
        }
    }
}

/// Resolve a dotted path like `realm_access.roles` against a JSON value.
pub fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for segment in path.split('.') {
        cur = cur.get(segment)?;
    }
    Some(cur)
}

pub fn get_string(v: &Value, path: &str) -> Option<String> {
    get_path(v, path).and_then(|v| v.as_str().map(ToString::to_string))
}

pub fn get_string_vec(v: &Value, path: &str) -> Vec<String> {
    match get_path(v, path) {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| x.as_str().map(ToString::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_keycloak_shape() {
        let c = json!({
            "sub": "alice",
            "tenant": "acme",
            "scope": "openid profile",
            "realm_access": { "roles": ["mcp.engineer", "user"] },
            "resource_access": { "mcp-gateway": { "roles": ["admin"] } },
            "groups": ["/eng"]
        });
        let ex = ClaimExtractor {
            role_paths: vec![
                "realm_access.roles".into(),
                "resource_access.mcp-gateway.roles".into(),
            ],
            ..Default::default()
        };
        let u = ex.extract(&c);
        assert_eq!(u.sub, "alice");
        assert_eq!(u.tenant.as_deref(), Some("acme"));
        assert!(u.roles.contains(&"mcp.engineer".to_string()));
        assert!(u.roles.contains(&"admin".to_string()));
        assert!(u.scopes.iter().any(|s| s == "openid"));
        assert!(u.groups.iter().any(|g| g == "/eng"));
    }
}
