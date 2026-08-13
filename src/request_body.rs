use std::io::{Seek, SeekFrom};

use axum::body::Body;
use futures_util::StreamExt;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::error::GatewayError;

const MAX_UPSTREAM_ERROR_BYTES: usize = 64 * 1024;
pub(crate) const MAX_REQUEST_BYTES: usize = 256 * 1024 * 1024;

pub(crate) async fn parse_json(body: Body) -> Result<Value, GatewayError> {
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
    file.seek(SeekFrom::Start(0)).await?;
    let file = file.into_std().await;
    tokio::task::spawn_blocking(move || serde_json::from_reader(file))
        .await
        .map_err(|error| GatewayError::Other(error.into()))?
        .map_err(GatewayError::Json)
}

pub(crate) async fn send_json<T>(
    request: reqwest::RequestBuilder,
    value: T,
) -> Result<reqwest::Response, GatewayError>
where
    T: Serialize + Send + 'static,
{
    let (file, length) = tokio::task::spawn_blocking(move || {
        let mut file = tempfile::tempfile()?;
        serde_json::to_writer(&mut file, &value)?;
        let length = file.stream_position()?;
        file.seek(SeekFrom::Start(0))?;
        Ok::<_, anyhow::Error>((file, length))
    })
    .await
    .map_err(|error| GatewayError::Other(error.into()))?
    .map_err(GatewayError::Other)?;
    let content_length = HeaderValue::from_str(&length.to_string())
        .map_err(|error| GatewayError::Other(error.into()))?;
    request
        .header(CONTENT_TYPE, "application/json")
        .header(CONTENT_LENGTH, content_length)
        .body(tokio::fs::File::from_std(file))
        .send()
        .await
        .map_err(GatewayError::Http)
}

pub(crate) async fn read_error_text(response: reqwest::Response) -> Result<String, GatewayError> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(GatewayError::Http)?;
        let remaining = MAX_UPSTREAM_ERROR_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() == MAX_UPSTREAM_ERROR_BYTES {
            truncated = stream.next().await.is_some();
            break;
        }
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        text.push_str(" [truncated]");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_request_bodies_above_the_explicit_limit() {
        let body = Body::from("12345");
        assert!(matches!(
            parse_json_with_limit(body, 4).await,
            Err(GatewayError::PayloadTooLarge)
        ));
    }
}
