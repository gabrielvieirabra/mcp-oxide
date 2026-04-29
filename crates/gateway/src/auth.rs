//! Axum extractor that validates the bearer token and yields a `UserContext`.

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use mcp_oxide_core::identity::UserContext;
use serde_json::json;

use crate::state::AppState;

#[derive(Debug)]
pub struct AuthUser(pub UserContext);

impl<S> FromRequestParts<S> for AuthUser
where
    AppState: axum::extract::FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app: AppState = AppState::from_ref(state);

        let header_value = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthRejection::missing())?;

        let token = header_value
            .strip_prefix("Bearer ")
            .or_else(|| header_value.strip_prefix("bearer "))
            .ok_or_else(AuthRejection::malformed)?;

        let user = app
            .identity
            .validate(token)
            .await
            .map_err(AuthRejection::unauthenticated)?;
        Ok(AuthUser(user))
    }
}

#[derive(Debug)]
pub struct AuthRejection {
    pub message: String,
}

impl AuthRejection {
    fn missing() -> Self {
        Self {
            message: "missing bearer token".into(),
        }
    }
    fn malformed() -> Self {
        Self {
            message: "malformed Authorization header".into(),
        }
    }
    fn unauthenticated<E: std::fmt::Display>(e: E) -> Self {
        Self {
            message: format!("{e}"),
        }
    }
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": "unauthenticated", "message": self.message }));
        (StatusCode::UNAUTHORIZED, body).into_response()
    }
}
