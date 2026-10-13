use std::io::{Seek, Write};

use futures_util::StreamExt;
use reqwest::header::CONTENT_TYPE;
#[cfg(test)]
use reqwest::header::{CONTENT_LENGTH, HeaderValue};
use serde::Serialize;

use crate::error::GatewayError;

const MAX_UPSTREAM_ERROR_BYTES: usize = 64 * 1024;
const MIN_COMPRESS_REQUEST_BYTES: u64 = 64 * 1024;

pub(super) struct JsonFile {
    pub(super) file: tokio::fs::File,
    pub(super) length: u64,
    pub(super) sha256: Option<String>,
}

pub(super) struct PreparedJson {
    pub(super) raw: JsonFile,
    pub(super) gzip: Option<JsonFile>,
}

struct DigestWriter<W> {
    inner: W,
    digest: Option<ring::digest::Context>,
}

impl<W: Write> Write for DigestWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(bytes)?;
        if let Some(digest) = &mut self.digest {
            digest.update(&bytes[..written]);
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn finish_json_file(mut writer: DigestWriter<std::fs::File>) -> anyhow::Result<JsonFile> {
    writer.flush()?;
    let length = writer.inner.metadata()?.len();
    writer.inner.rewind()?;
    let sha256 = writer.digest.map(|digest| {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let digest = digest.finish();
        let mut hex = String::with_capacity(digest.as_ref().len() * 2);
        for byte in digest.as_ref() {
            hex.push(char::from(HEX[usize::from(*byte >> 4)]));
            hex.push(char::from(HEX[usize::from(*byte & 0x0f)]));
        }
        hex
    });
    Ok(JsonFile {
        file: tokio::fs::File::from_std(writer.inner),
        length,
        sha256,
    })
}

pub(super) async fn prepare_json_body<T>(
    value: T,
    compress: bool,
    signed: bool,
) -> Result<PreparedJson, GatewayError>
where
    T: Serialize + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut writer = DigestWriter {
            inner: tempfile::tempfile()?,
            digest: signed.then(|| ring::digest::Context::new(&ring::digest::SHA256)),
        };
        serde_json::to_writer(&mut writer, &value)?;
        writer.flush()?;
        let length = writer.inner.stream_position()?;
        writer.inner.rewind()?;
        let gzip = if compress && length >= MIN_COMPRESS_REQUEST_BYTES {
            let compressed = DigestWriter {
                inner: tempfile::tempfile()?,
                digest: signed.then(|| ring::digest::Context::new(&ring::digest::SHA256)),
            };
            // Disk-backed encoding bounds memory even for long coding contexts.
            let mut encoder =
                flate2::write::GzEncoder::new(compressed, flate2::Compression::fast());
            std::io::copy(&mut writer.inner, &mut encoder)?;
            let compressed = finish_json_file(encoder.finish()?)?;
            (compressed.length < length).then_some(compressed)
        } else {
            None
        };
        Ok::<_, anyhow::Error>(PreparedJson {
            raw: finish_json_file(writer)?,
            gzip,
        })
    })
    .await
    .map_err(|error| GatewayError::Other(error.into()))?
    .map_err(GatewayError::Other)
}

#[cfg(test)]
pub(crate) async fn prepare_json<T>(
    request: reqwest::RequestBuilder,
    value: T,
) -> Result<reqwest::RequestBuilder, GatewayError>
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
    Ok(request
        .header(CONTENT_TYPE, "application/json")
        .header(CONTENT_LENGTH, content_length)
        .body(tokio::fs::File::from_std(file)))
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

pub(crate) async fn response_error(
    response: reqwest::Response,
    context: impl Into<String>,
) -> Result<GatewayError, GatewayError> {
    let status = response.status();
    let content_type = response.headers().get(CONTENT_TYPE).cloned();
    let body = read_error_text(response).await?;
    Ok(GatewayError::UpstreamStatus {
        status,
        body,
        content_type,
        context: context.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn compressed_json_preserves_payload_and_signed_bytes() {
        let value = serde_json::json!({"input": "coding context ".repeat(10_000)});
        let prepared = prepare_json_body(value.clone(), true, true).await.unwrap();
        let mut gzip = prepared.gzip.unwrap();
        let mut bytes = Vec::new();
        gzip.file.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(gzip.length as usize, bytes.len());
        let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
        let expected: String = digest
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(gzip.sha256.as_deref(), Some(expected.as_str()));
        let mut restored = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .read_to_end(&mut restored)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&restored).unwrap(),
            value
        );
        assert!(gzip.length < prepared.raw.length);
    }

    #[tokio::test]
    async fn small_or_unsupported_requests_remain_uncompressed() {
        let value = serde_json::json!({"input":"hi"});
        let mut small = prepare_json_body(value.clone(), true, false).await.unwrap();
        assert!(small.gzip.is_none());
        let mut bytes = Vec::new();
        small.raw.file.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(small.raw.length as usize, bytes.len());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            value
        );
        let prepared = prepare_json_body(
            serde_json::json!({"input":"context".repeat(20_000)}),
            false,
            false,
        )
        .await
        .unwrap();
        assert!(prepared.gzip.is_none());
        assert!(prepared.raw.sha256.is_none());
    }
}
