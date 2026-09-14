use axum::body::Body;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::{GatewayError, format_error_chain};

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        log_gateway_error(&self);
        let error = match self {
            GatewayError::UpstreamStatus {
                status,
                body,
                content_type,
                ..
            } => {
                let mut response = Response::new(Body::from(body));
                *response.status_mut() = status;
                if let Some(content_type) = content_type {
                    response
                        .headers_mut()
                        .insert(header::CONTENT_TYPE, content_type);
                }
                return response;
            }
            error => error,
        };
        let (status, message) = match error {
            GatewayError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            GatewayError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            GatewayError::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body is too large".to_owned(),
            ),
            GatewayError::Upstream(message) => (StatusCode::BAD_GATEWAY, message),
            GatewayError::UpstreamStatus { .. } => unreachable!("handled above"),
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

fn log_gateway_error(error: &GatewayError) {
    let error_chain = format_error_chain(error);
    match error {
        GatewayError::Unauthorized => {}
        GatewayError::BadRequest(_) | GatewayError::PayloadTooLarge | GatewayError::Json(_) => {
            tracing::warn!(error = %error_chain, "gateway request rejected");
        }
        GatewayError::Upstream(_) | GatewayError::UpstreamStatus { .. } | GatewayError::Http(_) => {
            tracing::error!(error = %error_chain, "gateway upstream request failed");
        }
        GatewayError::Io(_) | GatewayError::Other(_) => {
            tracing::error!(error = %error_chain, "gateway request failed");
        }
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
    async fn upstream_status_and_body_pass_through() {
        let upstream_body = r#"{"error":{"message":"provider overloaded","type":"429001"}}"#;
        let response = GatewayError::UpstreamStatus {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: upstream_body.to_owned(),
            content_type: Some("application/json".parse().unwrap()),
            context: "provider test responses endpoint".to_owned(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, upstream_body);
    }
}
