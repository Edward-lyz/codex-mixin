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
            GatewayError::Forbidden(message) => (StatusCode::FORBIDDEN, message),
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
        GatewayError::BadRequest(_)
        | GatewayError::Forbidden(_)
        | GatewayError::PayloadTooLarge
        | GatewayError::Json(_) => {
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
