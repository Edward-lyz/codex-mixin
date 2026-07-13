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
        if matches!(&self, GatewayError::Io(_) | GatewayError::Other(_)) {
            tracing::error!(error = %self, "gateway request failed");
        }
        let (status, message) = match &self {
            GatewayError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.clone()),
            GatewayError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            GatewayError::Upstream(message) => (StatusCode::BAD_GATEWAY, message.clone()),
            GatewayError::Http(err) => (StatusCode::BAD_GATEWAY, err.to_string()),
            GatewayError::Io(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_owned(),
            ),
            GatewayError::Json(err) => (StatusCode::BAD_REQUEST, err.to_string()),
            GatewayError::Other(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_owned(),
            ),
        };
        (status, axum::Json(json!({"error": {"message": message}}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn internal_errors_do_not_expose_details() {
        for error in [
            GatewayError::Io(std::io::Error::other("secret /private/path")),
            GatewayError::Other(anyhow::anyhow!("secret internal topology")),
        ] {
            let response = error.into_response();
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                json!({"error":{"message":"internal server error"}})
            );
        }
    }
}
