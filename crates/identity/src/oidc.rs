//! Generic OIDC identity provider.
//!
//! * Discovers the issuer metadata at `/.well-known/openid-configuration`.
//! * Fetches and caches JWKS with background refresh.
//! * Validates JWTs against cached keys (alg allowlist, iss/aud/exp/nbf).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use mcp_oxide_core::{identity::UserContext, providers::IdProvider, Error, Result};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::Value;

use crate::claims::ClaimExtractor;

#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub audiences: Vec<String>,
    pub algorithms: Vec<Algorithm>,
    pub jwks_cache_ttl: Duration,
    pub clock_skew_s: u64,
    pub http_timeout: Duration,
    pub extractor: ClaimExtractor,
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            issuer: String::new(),
            audiences: Vec::new(),
            algorithms: vec![Algorithm::RS256, Algorithm::ES256],
            jwks_cache_ttl: Duration::from_secs(300),
            clock_skew_s: 30,
            http_timeout: Duration::from_secs(5),
            extractor: ClaimExtractor::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DiscoveryDoc {
    issuer: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize, Clone)]
struct Jwk {
    kid: Option<String>,
    alg: Option<String>,
    #[serde(rename = "use")]
    key_use: Option<String>,
    #[serde(default)]
    kty: String,
    // RSA
    n: Option<String>,
    e: Option<String>,
    // EC
    #[allow(dead_code)]
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
    // OKP
    #[serde(default)]
    x_okp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Debug)]
struct Cached {
    keys: Vec<CachedKey>,
    fetched_at: std::time::Instant,
}

#[derive(Clone)]
struct CachedKey {
    kid: Option<String>,
    algorithm: Algorithm,
    decoding: Arc<DecodingKey>,
}

impl std::fmt::Debug for CachedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedKey")
            .field("kid", &self.kid)
            .field("algorithm", &self.algorithm)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct OidcProvider {
    cfg: OidcConfig,
    http: reqwest::Client,
    jwks_uri: String,
    issuer: String,
    cache: RwLock<Option<Cached>>,
}

impl OidcProvider {
    /// Discover the issuer and initialise the JWKS cache.
    pub async fn connect(cfg: OidcConfig) -> Result<Self> {
        if cfg.issuer.is_empty() {
            return Err(Error::Internal("oidc: empty issuer".into()));
        }
        let http = reqwest::Client::builder()
            .timeout(cfg.http_timeout)
            .build()
            .map_err(|e| Error::Internal(format!("oidc http client: {e}")))?;

        let disc_url = format!(
            "{}/.well-known/openid-configuration",
            cfg.issuer.trim_end_matches('/')
        );
        let disc: DiscoveryDoc = http
            .get(&disc_url)
            .send()
            .await
            .map_err(|e| Error::UpstreamUnavailable(format!("oidc discovery: {e}")))?
            .error_for_status()
            .map_err(|e| Error::UpstreamUnavailable(format!("oidc discovery: {e}")))?
            .json()
            .await
            .map_err(|e| Error::Internal(format!("oidc discovery parse: {e}")))?;

        if disc.issuer.trim_end_matches('/') != cfg.issuer.trim_end_matches('/') {
            return Err(Error::Internal(format!(
                "oidc issuer mismatch: configured={} discovered={}",
                cfg.issuer, disc.issuer
            )));
        }

        let provider = Self {
            cfg,
            http,
            jwks_uri: disc.jwks_uri,
            issuer: disc.issuer,
            cache: RwLock::new(None),
        };
        provider.refresh_keys_inner().await?;
        Ok(provider)
    }

    async fn refresh_keys_inner(&self) -> Result<()> {
        let set: JwkSet = self
            .http
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|e| Error::UpstreamUnavailable(format!("jwks: {e}")))?
            .error_for_status()
            .map_err(|e| Error::UpstreamUnavailable(format!("jwks: {e}")))?
            .json()
            .await
            .map_err(|e| Error::Internal(format!("jwks parse: {e}")))?;

        let mut keys = Vec::with_capacity(set.keys.len());
        for jwk in set.keys {
            if let Some(use_) = &jwk.key_use {
                if use_ != "sig" {
                    continue;
                }
            }
            if let Some(k) = convert_jwk(&jwk) {
                if self.cfg.algorithms.contains(&k.algorithm) {
                    keys.push(k);
                }
            }
        }
        if keys.is_empty() {
            return Err(Error::Internal(
                "jwks: no compatible signing keys after filtering".into(),
            ));
        }
        *self.cache.write() = Some(Cached {
            keys,
            fetched_at: std::time::Instant::now(),
        });
        tracing::info!(
            count = self.cache.read().as_ref().unwrap().keys.len(),
            "jwks refreshed"
        );
        Ok(())
    }

    fn lookup(&self, kid: Option<&str>, alg: Algorithm) -> Option<CachedKey> {
        let guard = self.cache.read();
        let cached = guard.as_ref()?;
        cached
            .keys
            .iter()
            .find(|k| k.algorithm == alg && kid.is_none_or(|id| k.kid.as_deref() == Some(id)))
            .cloned()
    }

    fn cache_stale(&self) -> bool {
        self.cache
            .read()
            .as_ref()
            .is_none_or(|c| c.fetched_at.elapsed() > self.cfg.jwks_cache_ttl)
    }

    fn make_validation(&self, alg: Algorithm) -> Validation {
        let mut v = Validation::new(alg);
        v.leeway = self.cfg.clock_skew_s;
        v.set_issuer(&[&self.issuer]);
        if self.cfg.audiences.is_empty() {
            v.validate_aud = false;
        } else {
            v.set_audience(&self.cfg.audiences);
        }
        v
    }
}

#[async_trait]
impl IdProvider for OidcProvider {
    async fn validate(&self, token: &str) -> Result<UserContext> {
        let header =
            decode_header(token).map_err(|e| Error::Unauthenticated(format!("jwt header: {e}")))?;

        // alg allowlist.
        if !self.cfg.algorithms.contains(&header.alg) {
            return Err(Error::Unauthenticated(format!(
                "jwt: algorithm {:?} not allowed",
                header.alg
            )));
        }

        // kid rotation: try cache → refresh once → fail.
        let mut key = self.lookup(header.kid.as_deref(), header.alg);
        if key.is_none() || self.cache_stale() {
            // Best-effort refresh, ignore non-kid-related errors.
            let _ = self.refresh_keys_inner().await;
            key = self.lookup(header.kid.as_deref(), header.alg);
        }
        let key = key.ok_or_else(|| {
            Error::Unauthenticated(format!("jwt: unknown signing key (kid={:?})", header.kid))
        })?;

        let validation = self.make_validation(header.alg);
        let data = decode::<Value>(token, &key.decoding, &validation)
            .map_err(|e| Error::Unauthenticated(format!("jwt: {e}")))?;

        Ok(self.cfg.extractor.extract(&data.claims))
    }

    async fn refresh_keys(&self) -> Result<()> {
        self.refresh_keys_inner().await
    }

    fn kind(&self) -> &'static str {
        "oidc-generic"
    }
}

fn convert_jwk(jwk: &Jwk) -> Option<CachedKey> {
    let alg = match jwk.alg.as_deref() {
        Some("RS256") => Algorithm::RS256,
        Some("RS384") => Algorithm::RS384,
        Some("RS512") => Algorithm::RS512,
        Some("ES256") => Algorithm::ES256,
        Some("ES384") => Algorithm::ES384,
        Some("EdDSA") => Algorithm::EdDSA,
        _ => {
            // Fall back based on kty when alg missing.
            match jwk.kty.as_str() {
                "RSA" => Algorithm::RS256,
                "EC" => Algorithm::ES256,
                "OKP" => Algorithm::EdDSA,
                _ => return None,
            }
        }
    };
    let decoding = match jwk.kty.as_str() {
        "RSA" => {
            let n = jwk.n.as_deref()?;
            let e = jwk.e.as_deref()?;
            DecodingKey::from_rsa_components(n, e).ok()?
        }
        "EC" => {
            let x = jwk.x.as_deref()?;
            let y = jwk.y.as_deref()?;
            DecodingKey::from_ec_components(x, y).ok()?
        }
        "OKP" => {
            let x = jwk.x.as_deref().or(jwk.x_okp.as_deref())?;
            DecodingKey::from_ed_components(x).ok()?
        }
        _ => return None,
    };
    Some(CachedKey {
        kid: jwk.kid.clone(),
        algorithm: alg,
        decoding: Arc::new(decoding),
    })
}
