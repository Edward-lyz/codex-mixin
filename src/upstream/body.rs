use std::io::{Seek, Write};

use futures_util::StreamExt;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use serde::Serialize;

use crate::error::GatewayError;

const MAX_UPSTREAM_ERROR_BYTES: usize = 64 * 1024;

pub(super) struct SignedJsonBody {
    pub(super) file: tokio::fs::File,
    pub(super) length: u64,
    pub(super) sha256: String,
}

struct DigestWriter<W> {
    inner: W,
    digest: ring::digest::Context,
}

impl<W: Write> Write for DigestWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.digest.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub(super) async fn prepare_signed_json<T>(value: T) -> Result<SignedJsonBody, GatewayError>
where
    T: Serialize + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let file = tempfile::tempfile()?;
        let mut writer = DigestWriter {
            inner: file,
            digest: ring::digest::Context::new(&ring::digest::SHA256),
        };
        serde_json::to_writer(&mut writer, &value)?;
        writer.flush()?;
        let length = writer.inner.stream_position()?;
        writer.inner.seek(std::io::SeekFrom::Start(0))?;
        let digest = writer.digest.finish();
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut sha256 = String::with_capacity(digest.as_ref().len() * 2);
        for byte in digest.as_ref() {
            sha256.push(char::from(HEX[usize::from(*byte >> 4)]));
            sha256.push(char::from(HEX[usize::from(*byte & 0x0f)]));
        }
        Ok::<_, anyhow::Error>(SignedJsonBody {
            file: tokio::fs::File::from_std(writer.inner),
            length,
            sha256,
        })
    })
    .await
    .map_err(|error| GatewayError::Other(error.into()))?
    .map_err(GatewayError::Other)
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
        file.seek(std::io::SeekFrom::Start(0))?;
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
