use std::io::{Seek, Write};

use futures_util::StreamExt;
use reqwest::header::CONTENT_TYPE;
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
