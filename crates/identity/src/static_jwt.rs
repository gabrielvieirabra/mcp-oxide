//! `static-jwt` provider: validates JWTs against a statically-configured
//! public key. Useful for development, tests, and trusted-mesh setups.

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use mcp_oxide_core::{identity::UserContext, providers::IdProvider, Error, Result};
use serde_json::Value;

use crate::claims::ClaimExtractor;

#[derive(Clone)]
pub struct StaticJwtConfig {
    pub algorithm: Algorithm,
    pub key: DecodingKey,
    pub issuer: Option<String>,
    pub audiences: Vec<String>,
    pub clock_skew_s: u64,
    pub extractor: ClaimExtractor,
}

impl std::fmt::Debug for StaticJwtConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticJwtConfig")
            .field("algorithm", &self.algorithm)
            .field("issuer", &self.issuer)
            .field("audiences", &self.audiences)
            .field("clock_skew_s", &self.clock_skew_s)
            .finish_non_exhaustive()
    }
}

pub struct StaticJwtProvider {
    algorithm: Algorithm,
    key: DecodingKey,
    validation: Validation,
    extractor: ClaimExtractor,
}

impl std::fmt::Debug for StaticJwtProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticJwtProvider")
            .field("algorithm", &self.algorithm)
            .finish_non_exhaustive()
    }
}

impl StaticJwtProvider {
    pub fn new(cfg: StaticJwtConfig) -> Self {
        let mut v = Validation::new(cfg.algorithm);
        v.leeway = cfg.clock_skew_s;
        if let Some(iss) = cfg.issuer.as_ref() {
            v.set_issuer(&[iss]);
        }
        if cfg.audiences.is_empty() {
            v.validate_aud = false;
        } else {
            v.set_audience(&cfg.audiences);
        }
        Self {
            algorithm: cfg.algorithm,
            key: cfg.key,
            validation: v,
            extractor: cfg.extractor,
        }
    }

    /// Convenience constructor from a PEM-encoded public key.
    pub fn from_pem(
        pem: &[u8],
        algorithm: Algorithm,
        issuer: Option<String>,
        audiences: Vec<String>,
    ) -> Result<Self> {
        let key = match algorithm {
            Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 => {
                DecodingKey::from_rsa_pem(pem)
                    .map_err(|e| Error::Internal(format!("jwt key: {e}")))?
            }
            Algorithm::ES256 | Algorithm::ES384 => DecodingKey::from_ec_pem(pem)
                .map_err(|e| Error::Internal(format!("jwt key: {e}")))?,
            Algorithm::EdDSA => DecodingKey::from_ed_pem(pem)
                .map_err(|e| Error::Internal(format!("jwt key: {e}")))?,
            _ => {
                return Err(Error::Internal(format!(
                    "unsupported alg for PEM: {algorithm:?}"
                )))
            }
        };
        Ok(Self::new(StaticJwtConfig {
            algorithm,
            key,
            issuer,
            audiences,
            clock_skew_s: 30,
            extractor: ClaimExtractor::default(),
        }))
    }
}

#[async_trait]
impl IdProvider for StaticJwtProvider {
    async fn validate(&self, token: &str) -> Result<UserContext> {
        let data = jsonwebtoken::decode::<Value>(token, &self.key, &self.validation)
            .map_err(|e| Error::Unauthenticated(format!("jwt: {e}")))?;
        Ok(self.extractor.extract(&data.claims))
    }

    fn kind(&self) -> &'static str {
        "static-jwt"
    }
}

impl StaticJwtProvider {
    #[must_use]
    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;

    #[tokio::test]
    async fn hs256_roundtrip() {
        let secret = b"super-secret-bytes-long-enough-for-tests";
        let key = DecodingKey::from_secret(secret);
        let p = StaticJwtProvider::new(StaticJwtConfig {
            algorithm: Algorithm::HS256,
            key,
            issuer: Some("test-iss".into()),
            audiences: vec!["test-aud".into()],
            clock_skew_s: 5,
            extractor: ClaimExtractor::default(),
        });

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = json!({
            "sub": "alice",
            "iss": "test-iss",
            "aud": "test-aud",
            "iat": now,
            "exp": now + 60,
            "roles": ["admin"]
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();

        let u = p.validate(&token).await.unwrap();
        assert_eq!(u.sub, "alice");
        assert!(u.roles.contains(&"admin".to_string()));
    }

    #[tokio::test]
    async fn rejects_bad_issuer() {
        let secret = b"super-secret-bytes-long-enough-for-tests";
        let key = DecodingKey::from_secret(secret);
        let p = StaticJwtProvider::new(StaticJwtConfig {
            algorithm: Algorithm::HS256,
            key,
            issuer: Some("expected".into()),
            audiences: vec!["test-aud".into()],
            clock_skew_s: 5,
            extractor: ClaimExtractor::default(),
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = json!({
            "sub": "x", "iss": "wrong", "aud": "test-aud",
            "iat": now, "exp": now+60
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap();
        let err = p.validate(&token).await.unwrap_err();
        matches!(err, Error::Unauthenticated(_));
    }
}
