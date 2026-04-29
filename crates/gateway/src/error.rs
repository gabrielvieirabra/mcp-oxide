//! HTTP error → response mapping.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use mcp_oxide_core::Error as CoreError;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AppError {
    #[error(transparent)]
    Core(#[from] CoreError),

    #[error("internal: {0}")]
    Internal(String),
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::Core(CoreError::Unauthenticated(_)) => {
                (StatusCode::UNAUTHORIZED, "unauthenticated")
            }
            AppError::Core(CoreError::Forbidden(_)) => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::Core(CoreError::NotFound(_)) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Core(CoreError::Conflict(_)) => (StatusCode::CONFLICT, "conflict"),
            AppError::Core(CoreError::InvalidRequest(_)) => {
                (StatusCode::BAD_REQUEST, "invalid_params")
            }
            AppError::Core(CoreError::RateLimited) => {
                (StatusCode::TOO_MANY_REQUESTS, "rate_limited")
            }
            AppError::Core(CoreError::UpstreamUnavailable(_)) => {
                (StatusCode::BAD_GATEWAY, "upstream_unavailable")
            }
            AppError::Core(CoreError::UpstreamTimeout(_)) => {
                (StatusCode::GATEWAY_TIMEOUT, "upstream_timeout")
            }
            AppError::Core(CoreError::Internal(_)) | AppError::Internal(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };

        let body = ErrorBody {
            error: code,
            message: self.to_string(),
        };
        (status, Json(body)).into_response()
    }
}
