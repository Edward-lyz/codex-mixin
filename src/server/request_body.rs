use axum::body::Body;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::error::GatewayError;

pub(super) const MAX_REQUEST_BYTES: usize = 256 * 1024 * 1024;

pub(super) async fn parse_json(body: Body) -> Result<Value, GatewayError> {
    parse_json_with_limit(body, MAX_REQUEST_BYTES).await
}

async fn parse_json_with_limit(body: Body, max_bytes: usize) -> Result<Value, GatewayError> {
    let file = tempfile::tempfile().map_err(GatewayError::Io)?;
    let mut file = tokio::fs::File::from_std(file);
    let mut stream = body.into_data_stream();
    let mut received = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            tracing::warn!(%error, "failed to read HTTP request body");
            GatewayError::BadRequest("invalid or truncated request body".to_owned())
        })?;
        received = received
            .checked_add(chunk.len())
            .ok_or(GatewayError::PayloadTooLarge)?;
        if received > max_bytes {
            return Err(GatewayError::PayloadTooLarge);
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    file.seek(std::io::SeekFrom::Start(0)).await?;
    let file = file.into_std().await;
    tokio::task::spawn_blocking(move || serde_json::from_reader(file))
        .await
        .map_err(|error| GatewayError::Other(error.into()))?
        .map_err(GatewayError::Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_request_bodies_above_the_explicit_limit() {
        assert!(matches!(
            parse_json_with_limit(Body::from("12345"), 4).await,
            Err(GatewayError::PayloadTooLarge)
        ));
    }

    #[tokio::test]
    async fn limits_the_decompressed_request_size() {
        use std::io::Write;
        use tower::ServiceExt;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder
            .write_all(format!("\"{}\"", "a".repeat(1000)).as_bytes())
            .unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < 64);
        let app = axum::Router::new()
            .route(
                "/",
                axum::routing::post(|body: Body| async {
                    parse_json_with_limit(body, 64)
                        .await
                        .map(|_| axum::http::StatusCode::OK)
                }),
            )
            .layer(tower_http::decompression::RequestDecompressionLayer::new());
        let response = app
            .oneshot(
                axum::http::Request::post("/")
                    .header("content-encoding", "gzip")
                    .body(Body::from(compressed))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }
}
