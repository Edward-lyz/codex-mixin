use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            GatewayError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.clone()),
            GatewayError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            GatewayError::Upstream(message) => (StatusCode::BAD_GATEWAY, message.clone()),
            GatewayError::Http(err) => (StatusCode::BAD_GATEWAY, err.to_string()),
            GatewayError::Io(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            GatewayError::Json(err) => (StatusCode::BAD_REQUEST, err.to_string()),
            GatewayError::Other(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
        };
        (status, axum::Json(json!({"error": {"message": message}}))).into_response()
    }
}
