//! End-to-end data-plane tests: auth → policy → proxy → mock upstream.
//!
//! Exercises the real router built by `mcp_oxide_gateway::router`, with
//! static-jwt identity, YAML RBAC policy, and a trivial in-process upstream.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::post, Json, Router};
use http::StatusCode;
use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use mcp_oxide_authz::YamlRbacEngine;
use mcp_oxide_core::providers::{IdProvider, PolicyEngine};
use mcp_oxide_gateway::{router, state::ResolvedAdapter, AppState};
use mcp_oxide_identity::{claims::ClaimExtractor, StaticJwtConfig, StaticJwtProvider};
use serde_json::{json, Value};
use tokio::net::TcpListener;

const HS_SECRET: &[u8] = b"unit-test-static-jwt-secret-bytes-at-least-32b";
const ISSUER: &str = "test-iss";
const AUD: &str = "mcp-oxide-test";

fn make_token(sub: &str, roles: &[&str]) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({
        "sub": sub,
        "iss": ISSUER,
        "aud": AUD,
        "iat": now,
        "exp": now + 60,
        "roles": roles,
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(HS_SECRET),
    )
    .unwrap()
}

async fn spawn_upstream() -> SocketAddr {
    let app: Router = Router::new().route(
        "/mcp",
        post(
            |headers: http::HeaderMap, Json(body): Json<Value>| async move {
                let echo = json!({
                    "jsonrpc": "2.0",
                    "id": body.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "echoed_method": body.get("method"),
                        // Must be false — gateway must not leak client token.
                        "forwarded_auth": headers.contains_key("authorization"),
                    },
                });
                (StatusCode::OK, Json(echo))
            },
        ),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn spawn_gateway(adapter_upstream: &str) -> SocketAddr {
    let identity: Arc<dyn IdProvider> = Arc::new(StaticJwtProvider::new(StaticJwtConfig {
        algorithm: Algorithm::HS256,
        key: DecodingKey::from_secret(HS_SECRET),
        issuer: Some(ISSUER.into()),
        audiences: vec![AUD.into()],
        clock_skew_s: 5,
        extractor: ClaimExtractor::default(),
    }));

    let authz: Arc<dyn PolicyEngine> = Arc::new(
        YamlRbacEngine::from_str(
            r#"
version: 1
default: deny
rules:
  - plane: data
    action: "tools/call"
    target: "demo"
    allow_roles: ["mcp.engineer"]
  - plane: data
    action: "tools/list"
    allow_roles: ["*"]
"#,
            "test",
        )
        .unwrap(),
    );

    let state = AppState::builder()
        .identity(identity)
        .authz(authz)
        .adapter(ResolvedAdapter {
            name: "demo".into(),
            upstream: adapter_upstream.to_string(),
            required_roles: vec!["mcp.engineer".into()],
            tags: vec!["test".into()],
        })
        .build()
        .unwrap();

    let app = router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn allowed_request_is_proxied() {
    let up_addr = spawn_upstream().await;
    let up_url = format!("http://{up_addr}/mcp");
    let gw = spawn_gateway(&up_url).await;

    let token = make_token("alice", &["mcp.engineer"]);
    let resp = client()
        .post(format!("http://{gw}/adapters/demo/mcp"))
        .bearer_auth(token)
        .json(&json!({"jsonrpc":"2.0","method":"tools/call","id":7}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["result"]["echoed_method"], "tools/call");
    // Gateway must NOT have forwarded the bearer token upstream.
    assert_eq!(body["result"]["forwarded_auth"], false);
}

#[tokio::test]
async fn denied_without_role() {
    let up_addr = spawn_upstream().await;
    let up_url = format!("http://{up_addr}/mcp");
    let gw = spawn_gateway(&up_url).await;

    let token = make_token("bob", &["viewer"]); // lacks mcp.engineer
    let resp = client()
        .post(format!("http://{gw}/adapters/demo/mcp"))
        .bearer_auth(token)
        .json(&json!({"jsonrpc":"2.0","method":"tools/call","id":7}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn rejected_without_token() {
    let gw = spawn_gateway("http://127.0.0.1:1").await;
    let resp = client()
        .post(format!("http://{gw}/adapters/demo/mcp"))
        .json(&json!({"jsonrpc":"2.0","method":"tools/call","id":7}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn rejects_bad_signature() {
    let gw = spawn_gateway("http://127.0.0.1:1").await;
    // Sign with a different secret.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({ "sub":"x", "iss":ISSUER, "aud":AUD, "iat":now, "exp":now+60 });
    let bad = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"some-other-secret-at-least-32-bytes-ok"),
    )
    .unwrap();

    let resp = client()
        .post(format!("http://{gw}/adapters/demo/mcp"))
        .bearer_auth(bad)
        .json(&json!({"jsonrpc":"2.0","method":"tools/call","id":7}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn unknown_adapter_returns_404() {
    let gw = spawn_gateway("http://127.0.0.1:1").await;
    let token = make_token("alice", &["mcp.engineer"]);
    let resp = client()
        .post(format!("http://{gw}/adapters/missing/mcp"))
        .bearer_auth(token)
        .json(&json!({"jsonrpc":"2.0","method":"tools/call","id":7}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn upstream_unavailable_maps_to_502() {
    // Point at a TCP endpoint guaranteed to refuse (127.0.0.1:1).
    let gw = spawn_gateway("http://127.0.0.1:1/mcp").await;
    let token = make_token("alice", &["mcp.engineer"]);
    let resp = client()
        .post(format!("http://{gw}/adapters/demo/mcp"))
        .bearer_auth(token)
        .json(&json!({"jsonrpc":"2.0","method":"tools/call","id":7}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "upstream_unavailable");
}

#[tokio::test]
async fn tools_list_wildcard_role() {
    let up_addr = spawn_upstream().await;
    let up_url = format!("http://{up_addr}/mcp");
    let gw = spawn_gateway(&up_url).await;

    // "tools/list" has "*" in allow_roles — but adapter still requires
    // mcp.engineer via required_roles, so a pure-viewer must still be denied.
    let viewer = make_token("v", &["viewer"]);
    let resp = client()
        .post(format!("http://{gw}/adapters/demo/mcp"))
        .bearer_auth(viewer)
        .json(&json!({"jsonrpc":"2.0","method":"tools/list","id":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Engineer passes both policy + required_roles.
    let eng = make_token("e", &["mcp.engineer"]);
    let resp = client()
        .post(format!("http://{gw}/adapters/demo/mcp"))
        .bearer_auth(eng)
        .json(&json!({"jsonrpc":"2.0","method":"tools/list","id":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
