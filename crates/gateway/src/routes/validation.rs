//! Input validation shared by control-plane handlers.
//!
//! - Resource names (adapters, tools) must be DNS-label-ish so they are safe
//!   to use as container names, Kubernetes resource names, URL path segments,
//!   and HTTP header values.
//! - User-supplied environment variable names must not collide with sensitive
//!   prefixes that could be exploited to exfiltrate data or hijack execution.

use mcp_oxide_core::Error;

/// Maximum resource-name length. 63 is the Kubernetes label limit, which is
/// the most restrictive target for any future deployment provider.
pub const MAX_NAME_LEN: usize = 63;

/// Validate a control-plane resource name (adapter or tool).
///
/// Accepts `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$` — a lowercase DNS label.
/// This forbids path traversal (`/`, `.`), shell metacharacters, leading or
/// trailing hyphens, and anything longer than 63 chars.
pub fn validate_resource_name(name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::InvalidRequest("name must not be empty".into()));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(Error::InvalidRequest(format!(
            "name must be <= {MAX_NAME_LEN} chars"
        )));
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(Error::InvalidRequest(
            "name must start with [a-z0-9]".into(),
        ));
    }
    if !bytes[bytes.len() - 1].is_ascii_lowercase() && !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err(Error::InvalidRequest("name must end with [a-z0-9]".into()));
    }
    for &b in bytes {
        if !(b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err(Error::InvalidRequest(
                "name must match ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$".into(),
            ));
        }
    }
    Ok(())
}

/// Environment-variable name prefixes that are refused from user input because
/// they can be abused to change process behaviour, hijack execution, or leak
/// credentials from the gateway to the workload container.
pub const DISALLOWED_ENV_PREFIXES: &[&str] = &[
    "LD_",          // LD_PRELOAD / LD_LIBRARY_PATH — code injection
    "DYLD_",        // macOS equivalent
    "DOCKER_",      // DOCKER_HOST — hijack docker client
    "KUBERNETES_",  // service-account tokens path
    "AWS_",         // cloud creds propagation
    "GOOGLE_",      // GOOGLE_APPLICATION_CREDENTIALS
    "AZURE_",       // azure creds
    "PATH",         // entrypoint hijack
    "PYTHONPATH",   // python module hijack
    "NODE_OPTIONS", // node boot flags
];

/// Validate a user-supplied env-var name. Rejects empty names, names
/// containing `=` (invalid), and names starting with any disallowed prefix.
pub fn validate_env_var_name(name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::InvalidRequest("env name must not be empty".into()));
    }
    if name.contains('=') {
        return Err(Error::InvalidRequest("env name must not contain '='".into()));
    }
    if name.contains(char::is_whitespace) {
        return Err(Error::InvalidRequest(
            "env name must not contain whitespace".into(),
        ));
    }
    for prefix in DISALLOWED_ENV_PREFIXES {
        if name == *prefix || name.starts_with(prefix) {
            return Err(Error::InvalidRequest(format!(
                "env name '{name}' uses reserved prefix '{prefix}'"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        for ok in ["mcp-aws", "tool1", "a", "a-b-c-123", "abc"] {
            validate_resource_name(ok).expect(ok);
        }
    }

    #[test]
    fn rejects_bad_names() {
        for bad in [
            "",
            "Foo",
            "-bad",
            "bad-",
            "a_b",
            "a/b",
            "a.b",
            "../etc/passwd",
            "foo; rm -rf /",
            &"x".repeat(64),
        ] {
            assert!(validate_resource_name(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn env_rejects_reserved() {
        for bad in ["LD_PRELOAD", "DOCKER_HOST", "AWS_ACCESS_KEY_ID", "PATH"] {
            assert!(validate_env_var_name(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn env_accepts_user() {
        for ok in ["LOG_LEVEL", "MY_TOKEN", "FEATURE_FLAG_X"] {
            validate_env_var_name(ok).expect(ok);
        }
    }

    #[test]
    fn env_rejects_invalid_shape() {
        for bad in ["", "FOO=BAR", "A B"] {
            assert!(validate_env_var_name(bad).is_err(), "should reject: {bad:?}");
        }
    }
}
