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
        let chunk = chunk.map_err(|error| GatewayError::Other(error.into()))?;
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
}
