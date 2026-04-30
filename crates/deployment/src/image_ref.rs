//! OCI image reference parsing.
//!
//! Handles the three common forms:
//! - `name` / `name:tag`                         (implicit library registry)
//! - `registry[:port]/name[:tag]`
//! - `registry[:port]/name@sha256:<digest>`      (digest-pinned)
//! - combinations with `@sha256:<digest>` suffix on any of the above
//!
//! The naive `splitn(2, ':')` approach breaks on both digest references and
//! registry-port references; this module is the single place both are
//! handled correctly.

use mcp_oxide_core::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    /// Fully-qualified name excluding tag/digest (e.g. `ghcr.io/owner/repo`).
    pub name: String,
    /// Tag if present (e.g. `v1.2.3`, `latest`).
    pub tag: Option<String>,
    /// Digest if present, including the `sha256:` (or other algo) prefix.
    pub digest: Option<String>,
}

impl ImageRef {
    /// Parse an OCI image reference.
    pub fn parse(reference: &str) -> Result<Self, Error> {
        if reference.is_empty() {
            return Err(Error::InvalidRequest("image reference is empty".into()));
        }

        // Split off digest first. Digest, if present, is always the last
        // component and always after `@`.
        let (remainder, digest) = match reference.rsplit_once('@') {
            Some((head, tail)) => {
                if tail.is_empty() {
                    return Err(Error::InvalidRequest(
                        "image reference ends with '@'".into(),
                    ));
                }
                if !tail.contains(':') {
                    return Err(Error::InvalidRequest(
                        "image digest must be '<algo>:<hex>'".into(),
                    ));
                }
                (head, Some(tail.to_string()))
            }
            None => (reference, None),
        };

        // Separate name from tag. The rule: the tag-separator is the *last*
        // colon that appears AFTER the last slash. Colons before the final
        // slash belong to a registry port.
        let last_slash = remainder.rfind('/');
        let search_from = last_slash.map_or(0, |i| i + 1);
        let (name, tag) = match remainder[search_from..].rfind(':') {
            Some(rel_idx) => {
                let idx = search_from + rel_idx;
                let tag = &remainder[idx + 1..];
                if tag.is_empty() {
                    return Err(Error::InvalidRequest(
                        "image tag must not be empty".into(),
                    ));
                }
                (remainder[..idx].to_string(), Some(tag.to_string()))
            }
            None => (remainder.to_string(), None),
        };

        if name.is_empty() {
            return Err(Error::InvalidRequest("image name must not be empty".into()));
        }

        Ok(Self { name, tag, digest })
    }

    /// Effective tag for docker pull: user-supplied or `latest`.
    #[must_use]
    pub fn effective_tag(&self) -> &str {
        self.tag.as_deref().unwrap_or("latest")
    }

    /// `true` if the reference pins to an immutable digest.
    #[must_use]
    pub fn is_digest_pinned(&self) -> bool {
        self.digest.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_name() {
        let r = ImageRef::parse("alpine").unwrap();
        assert_eq!(r.name, "alpine");
        assert_eq!(r.tag, None);
        assert_eq!(r.digest, None);
        assert_eq!(r.effective_tag(), "latest");
    }

    #[test]
    fn name_and_tag() {
        let r = ImageRef::parse("alpine:3.20").unwrap();
        assert_eq!(r.name, "alpine");
        assert_eq!(r.tag.as_deref(), Some("3.20"));
    }

    #[test]
    fn registry_and_name() {
        let r = ImageRef::parse("ghcr.io/owner/repo:v1").unwrap();
        assert_eq!(r.name, "ghcr.io/owner/repo");
        assert_eq!(r.tag.as_deref(), Some("v1"));
    }

    #[test]
    fn registry_with_port() {
        let r = ImageRef::parse("registry.local:5000/repo:v1").unwrap();
        assert_eq!(r.name, "registry.local:5000/repo");
        assert_eq!(r.tag.as_deref(), Some("v1"));
    }

    #[test]
    fn registry_with_port_no_tag() {
        let r = ImageRef::parse("registry.local:5000/repo").unwrap();
        assert_eq!(r.name, "registry.local:5000/repo");
        assert_eq!(r.tag, None);
    }

    #[test]
    fn digest_pinned() {
        let r = ImageRef::parse(
            "ghcr.io/owner/repo@sha256:deadbeefcafef00d",
        )
        .unwrap();
        assert_eq!(r.name, "ghcr.io/owner/repo");
        assert_eq!(r.tag, None);
        assert_eq!(r.digest.as_deref(), Some("sha256:deadbeefcafef00d"));
        assert!(r.is_digest_pinned());
    }

    #[test]
    fn tag_and_digest() {
        let r = ImageRef::parse(
            "ghcr.io/owner/repo:v1@sha256:deadbeefcafef00d",
        )
        .unwrap();
        assert_eq!(r.name, "ghcr.io/owner/repo");
        assert_eq!(r.tag.as_deref(), Some("v1"));
        assert_eq!(r.digest.as_deref(), Some("sha256:deadbeefcafef00d"));
    }

    #[test]
    fn registry_port_tag_digest() {
        let r = ImageRef::parse(
            "registry.local:5000/owner/repo:v1@sha256:abc",
        )
        .unwrap();
        assert_eq!(r.name, "registry.local:5000/owner/repo");
        assert_eq!(r.tag.as_deref(), Some("v1"));
        assert_eq!(r.digest.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn rejects_empty() {
        assert!(ImageRef::parse("").is_err());
    }

    #[test]
    fn rejects_trailing_at() {
        assert!(ImageRef::parse("alpine@").is_err());
    }

    #[test]
    fn rejects_bad_digest() {
        assert!(ImageRef::parse("alpine@notadigest").is_err());
    }

    #[test]
    fn rejects_empty_tag() {
        assert!(ImageRef::parse("alpine:").is_err());
    }
}
