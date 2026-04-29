//! Gateway HTTP router.

use axum::{
    routing::{get, post},
    Router,
};
use tower_http::request_id::MakeRequestUuid;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tower_http::ServiceBuilderExt;

use crate::{routes, state::AppState};

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/healthz", get(routes::health::healthz))
        .route("/healthz/startup", get(routes::health::startup))
        .route("/healthz/live", get(routes::health::live))
        .route("/healthz/ready", get(routes::health::ready))
        .route("/readyz", get(routes::health::ready))
        .route("/livez", get(routes::health::live))
        .route("/", get(routes::health::root))
        .route("/adapters/{name}/mcp", post(routes::data_plane::invoke))
        .with_state(state);

    let middleware = tower::ServiceBuilder::new()
        .set_x_request_id(MakeRequestUuid)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().include_headers(false))
                .on_response(DefaultOnResponse::new().include_headers(false)),
        )
        .propagate_x_request_id();

    Router::new().merge(api).layer(middleware)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_ok() {
        let cfg = Config::default();
        let state = AppState::bootstrap(&cfg).await.unwrap();
        let app = router(state);

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "ok");
        assert!(v["providers"].is_object());
    }

    #[tokio::test]
    async fn readyz_ok() {
        let cfg = Config::default();
        let state = AppState::bootstrap(&cfg).await.unwrap();
        let app = router(state);

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invoke_rejects_without_token() {
        let cfg = Config::default();
        let state = AppState::bootstrap(&cfg).await.unwrap();
        let app = router(state);

        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/adapters/anything/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"jsonrpc":"2.0","method":"ping","id":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
