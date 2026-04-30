//! Control-plane CRUD integration tests.

use std::sync::Arc;

use axum::http::StatusCode;
use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use mcp_oxide_authz::YamlRbacEngine;
use mcp_oxide_core::providers::{IdProvider, PolicyEngine};
use mcp_oxide_gateway::{router, AppState};
use mcp_oxide_identity::{claims::ClaimExtractor, StaticJwtConfig, StaticJwtProvider};
use serde_json::{json, Value};
use tower::ServiceExt;

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

fn build_state() -> AppState {
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
  - plane: control
    action: "adapters.*"
    allow_roles: ["mcp.admin"]
  - plane: control
    action: "adapters.read"
    allow_roles: ["mcp.viewer"]
  - plane: control
    action: "adapters.list"
    allow_roles: ["mcp.viewer"]
  - plane: control
    action: "tools.*"
    allow_roles: ["mcp.admin"]
  - plane: control
    action: "tools.read"
    allow_roles: ["mcp.viewer"]
  - plane: control
    action: "tools.list"
    allow_roles: ["mcp.viewer"]
"#,
            "test",
        )
        .unwrap(),
    );

    AppState::builder()
        .identity(identity)
        .authz(authz)
        .build()
        .unwrap()
}

#[tokio::test]
async fn create_adapter_success() {
    let state = build_state();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "test-adapter",
                        "image": "registry.example.com/test:1.0",
                        "endpoint_port": 8080,
                        "endpoint_path": "/mcp",
                        "replicas": 2,
                        "required_roles": ["mcp.engineer"],
                        "tags": ["test"]
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["name"], "test-adapter");
    assert_eq!(v["image"], "registry.example.com/test:1.0");
    assert_eq!(v["revision"], 1);
}

#[tokio::test]
async fn create_adapter_conflict() {
    let state = build_state();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    let body = serde_json::to_vec(&json!({
        "name": "duplicate",
        "image": "registry.example.com/test:1.0",
    }))
    .unwrap();

    let _ = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn get_adapter_not_found() {
    let state = build_state();
    let app = router(state);
    let token = make_token("viewer", &["mcp.viewer"]);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/adapters/nonexistent")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_adapter_revision_mismatch() {
    let state = build_state();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    let _ = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "rev-test",
                        "image": "registry.example.com/test:1.0",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/adapters/rev-test")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "description": "updated",
                        "revision": 999,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_adapter() {
    let state = build_state();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    let _ = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "to-delete",
                        "image": "registry.example.com/test:1.0",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("DELETE")
                .uri("/adapters/to-delete")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/adapters/to-delete")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_tool_success() {
    let state = build_state();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/tools")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "weather",
                        "image": "registry.example.com/weather:1.0",
                        "tool_definition": {
                            "name": "weather",
                            "description": "Get weather",
                            "input_schema": {
                                "type": "object",
                                "properties": {
                                    "location": { "type": "string" }
                                }
                            }
                        }
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["name"], "weather");
    assert_eq!(v["tool_definition"]["name"], "weather");
}

#[tokio::test]
async fn list_adapters_empty() {
    let state = build_state();
    let app = router(state);
    let token = make_token("viewer", &["mcp.viewer"]);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/adapters")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert!(v.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn control_plane_requires_auth() {
    let state = build_state();
    let app = router(state);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/adapters")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_admin_cannot_create_adapter() {
    let state = build_state();
    let app = router(state);
    let token = make_token("viewer", &["mcp.viewer"]);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "forbidden",
                        "image": "registry.example.com/test:1.0",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_returns_etag_header() {
    let state = build_state();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "etag-test",
                        "image": "registry.example.com/test:1.0",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let etag = resp.headers().get("etag").expect("etag header present");
    assert_eq!(etag, "W/\"1\"");
}

#[tokio::test]
async fn update_with_if_match_header_succeeds() {
    let state = build_state();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    let create = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "if-match-test",
                        "image": "registry.example.com/test:1.0",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let etag = create.headers().get("etag").unwrap().clone();

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/adapters/if-match-test")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .header("If-Match", etag.clone())
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({"description": "updated"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let new_etag = resp.headers().get("etag").expect("etag present");
    assert_eq!(new_etag, "W/\"2\"");
}

#[tokio::test]
async fn update_with_stale_if_match_returns_conflict() {
    let state = build_state();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    let _ = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "stale-test",
                        "image": "registry.example.com/test:1.0",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/adapters/stale-test")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .header("If-Match", "W/\"42\"")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({"description": "x"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn runtime_registered_adapter_is_routable() {
    // Mock upstream.
    let upstream_app = axum::Router::new().route(
        "/mcp",
        axum::routing::post(|| async { axum::Json(json!({"jsonrpc":"2.0","id":1,"result":"ok"})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, upstream_app).await.unwrap();
    });

    // Build a gateway with a policy that allows admin CRUD AND data-plane for
    // the runtime-registered adapter.
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
  - plane: control
    action: "adapters.*"
    allow_roles: ["mcp.admin"]
  - plane: data
    action: "tools/call"
    allow_roles: ["mcp.admin"]
"#,
            "test",
        )
        .unwrap(),
    );

    let state = AppState::builder()
        .identity(identity)
        .authz(authz)
        .build()
        .unwrap();
    let app = router(state);
    let token = make_token("admin", &["mcp.admin"]);

    // Register adapter at runtime.
    let upstream_url = format!("http://{upstream_addr}/mcp");
    let create = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "name": "runtime-adapter",
                        "image": "registry.example.com/test:1.0",
                        "upstream": upstream_url,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);

    // Invoke via data-plane — it must route to the runtime adapter without restart.
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/adapters/runtime-adapter/mcp")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"tools/call","id":1}))
                        .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "data plane should route to runtime adapter");
}
