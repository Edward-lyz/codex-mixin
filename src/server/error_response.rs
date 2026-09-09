use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::{GatewayError, format_error_chain};

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let error_chain = format_error_chain(&self);
        match &self {
            GatewayError::Unauthorized => {}
            GatewayError::BadRequest(_) | GatewayError::PayloadTooLarge | GatewayError::Json(_) => {
                tracing::warn!(error = %error_chain, "gateway request rejected");
            }
            GatewayError::Upstream(_)
            | GatewayError::UpstreamStatus { .. }
            | GatewayError::Http(_) => {
                tracing::error!(error = %error_chain, "gateway upstream request failed");
            }
            GatewayError::Io(_) | GatewayError::Other(_) => {
                tracing::error!(error = %error_chain, "gateway request failed");
            }
        }
        let (status, message) = match &self {
            GatewayError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.clone()),
            GatewayError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            GatewayError::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body is too large".to_owned(),
            ),
            GatewayError::Upstream(message) => (StatusCode::BAD_GATEWAY, message.clone()),
            GatewayError::UpstreamStatus { status, .. }
                if *status == StatusCode::PAYLOAD_TOO_LARGE =>
            {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "upstream request is too large".to_owned(),
                )
            }
            GatewayError::UpstreamStatus { .. } => (
                StatusCode::BAD_GATEWAY,
                "upstream request failed".to_owned(),
            ),
            GatewayError::Http(error) => (StatusCode::BAD_GATEWAY, error.to_string()),
            GatewayError::Io(_) | GatewayError::Other(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_owned(),
            ),
            GatewayError::Json(error) => (StatusCode::BAD_REQUEST, error.to_string()),
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

    #[tokio::test]
    async fn upstream_errors_only_preserve_payload_too_large_status() {
        let response = GatewayError::UpstreamStatus {
            status: StatusCode::UNAUTHORIZED,
            message: "secret provider account detail".to_owned(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            json!({"error":{"message":"upstream request failed"}})
        );
    }
}
