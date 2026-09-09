use axum::http::StatusCode;
use std::error::Error;

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("request body is too large")]
    PayloadTooLarge,
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("upstream returned {status}: {message}")]
    UpstreamStatus { status: StatusCode, message: String },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub fn format_error_chain(error: &(dyn Error + 'static)) -> String {
    let mut chain = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        chain.push_str(": ");
        chain.push_str(&error.to_string());
        source = error.source();
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_the_complete_error_chain_for_logs() {
        let error = GatewayError::Other(
            anyhow::anyhow!("connection refused").context("request provider models"),
        );

        assert_eq!(
            format_error_chain(&error),
            "request provider models: connection refused"
        );
    }
}
