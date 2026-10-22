//! HTTP error type.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("database error")]
    Database(#[from] sqlx::Error),
    /// Retryable: we could not store the request, through no fault of the
    /// caller. Distinct from BadRequest so a device knows to try again.
    #[error("service unavailable")]
    Unavailable,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            // 503, not 500: this is explicitly "try again", and the device's
            // retry with backoff is the correct response to it.
            AppError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        // Log server faults; don't leak internals to the client.
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!("server error: {self}");
        }
        let body = Json(json!({ "error": self.to_string() }));
        (status, body).into_response()
    }
}
