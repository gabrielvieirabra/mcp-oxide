//! JSON-RPC + SSE reverse proxy helpers.

use std::time::Instant;

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::TryStreamExt;
use sha2::{Digest, Sha256};

pub const HOP_BY_HOP: &[&str] = &[
    "connection",
    "proxy-connection",
    "keep-alive",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "host",
    "content-length",
];

/// Filter hop-by-hop headers before forwarding.
pub fn forwardable_headers(src: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in src {
        if HOP_BY_HOP
            .iter()
            .any(|h| k.as_str().eq_ignore_ascii_case(h))
        {
            continue;
        }
        if k == header::AUTHORIZATION {
            // Do not forward the end-user bearer token upstream; the gateway
            // has already authenticated. A future phase may forward a
            // gateway-minted token instead.
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Outcome of a proxy call that the caller needs for audit + metrics.
#[derive(Debug)]
pub struct ProxyOutcome {
    pub status: StatusCode,
    pub latency: std::time::Duration,
    #[allow(dead_code)] // surfaced as a label in Phase 4 metrics
    pub content_type: Option<String>,
}

/// Send `body` to `upstream` via POST, streaming the response body back as an
/// axum Response. Preserves content-type so SSE streams pass through.
pub async fn forward_post(
    client: &reqwest::Client,
    upstream: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(Response, ProxyOutcome), ProxyError> {
    let started = Instant::now();
    let req_headers = to_reqwest_headers(&headers);

    let resp = client
        .post(upstream)
        .headers(req_headers)
        .body(body.clone())
        .send()
        .await
        .map_err(|e| classify(&e))?;

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp_headers = resp.headers().clone();
    let content_type = resp_headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);

    let is_event_stream = content_type
        .as_deref()
        .is_some_and(|ct| ct.starts_with("text/event-stream"));

    // Stream the body; SSE (or large responses) are forwarded as a stream.
    let bytes_stream = resp.bytes_stream().map_err(std::io::Error::other);

    let mut out = Response::builder().status(status);
    let h = out.headers_mut().expect("builder");
    for (k, v) in &resp_headers {
        if HOP_BY_HOP
            .iter()
            .any(|x| k.as_str().eq_ignore_ascii_case(x))
        {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_str().as_bytes()),
            HeaderValue::from_bytes(v.as_bytes()),
        ) {
            h.append(name, value);
        }
    }
    if is_event_stream {
        // Prevent intermediary buffering.
        h.insert(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        );
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }

    let body = Body::from_stream(bytes_stream);
    let response = out.body(body).unwrap_or_else(|_| {
        (StatusCode::BAD_GATEWAY, "proxy: failed to build response").into_response()
    });

    Ok((
        response,
        ProxyOutcome {
            status,
            latency: started.elapsed(),
            content_type,
        },
    ))
}

fn to_reqwest_headers(src: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::new();
    for (k, v) in src {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(v.as_bytes()),
        ) {
            out.append(name, value);
        }
    }
    out
}

/// Classify an upstream request error into a normalized proxy error.
fn classify(e: &reqwest::Error) -> ProxyError {
    if e.is_timeout() {
        ProxyError::Timeout(e.to_string())
    } else {
        ProxyError::Unavailable(e.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("upstream unavailable: {0}")]
    Unavailable(String),
    #[error("upstream timeout: {0}")]
    Timeout(String),
}
