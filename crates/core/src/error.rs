//! Unified error type for mcp-oxide.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("upstream unavailable: {0}")]
    UpstreamUnavailable(String),

    #[error("upstream timeout: {0}")]
    UpstreamTimeout(String),

    #[error("rate limited")]
    RateLimited,

    #[error("internal: {0}")]
    Internal(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
