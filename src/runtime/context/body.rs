//! Lossless SQLite body encoding. This never changes model-facing JSON.
use std::io::Read;

use anyhow::{Context, Result, bail, ensure};
use rusqlite::types::Value;
use serde::Deserialize;

const MIN_COMPRESSION_BYTES: usize = 256;
const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

pub(super) struct EncodedBody {
    pub payload: Value,
    pub codec: i64,
    pub bytes: i64,
    pub checksum: Vec<u8>,
}

#[derive(Default, Deserialize)]
pub(super) struct BodyTags {
    pub role: Option<String>,
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    pub name: Option<String>,
}

pub(super) fn encode(raw: String, verify_migration: bool) -> Result<EncodedBody> {
    ensure!(
        raw.len() <= MAX_BODY_BYTES,
        "context body exceeds storage limit"
    );
    let checksum = ring::digest::digest(&ring::digest::SHA256, raw.as_bytes())
        .as_ref()
        .to_vec();
    let compressed = if raw.len() >= MIN_COMPRESSION_BYTES {
        Some(zstd::bulk::compress(raw.as_bytes(), 3).context("compress context body")?)
    } else {
        None
    };
    let bytes = raw.len() as i64;
    let (payload, codec) = match compressed {
        Some(compressed) if compressed.len() < raw.len() => {
            if verify_migration {
                let decoded = zstd::bulk::decompress(&compressed, raw.len())
                    .context("verify compressed context body")?;
                ensure!(
                    decoded == raw.as_bytes(),
                    "context body compression round trip failed"
                );
            }
            (Value::Blob(compressed), 1)
        }
        _ => (Value::Text(raw), 0),
    };
    let result = EncodedBody {
        payload,
        codec,
        bytes,
        checksum,
    };
    Ok(result)
}

pub(super) fn decode(
    payload: Value,
    codec: i64,
    bytes: i64,
    checksum: Option<&[u8]>,
) -> Result<String> {
    let raw = match (codec, payload) {
        (0, Value::Text(raw)) => raw,
        (1, Value::Blob(compressed)) => {
            let limit = usize::try_from(bytes).context("invalid context body length")?;
            ensure!(
                limit > 0 && limit <= MAX_BODY_BYTES,
                "invalid compressed context body length"
            );
            let decoder = zstd::stream::read::Decoder::new(compressed.as_slice())
                .context("open compressed context body")?;
            let mut raw = Vec::with_capacity(limit.min(64 * 1024));
            decoder
                .take(limit as u64 + 1)
                .read_to_end(&mut raw)
                .context("decode compressed context body")?;
            ensure!(raw.len() == limit, "context body length mismatch");
            String::from_utf8(raw).context("context body is not UTF-8")?
        }
        _ => bail!("unsupported context body encoding {codec}"),
    };
    if bytes > 0 {
        ensure!(raw.len() as i64 == bytes, "context body length mismatch");
    }
    if let Some(expected) = checksum {
        let actual = ring::digest::digest(&ring::digest::SHA256, raw.as_bytes());
        ensure!(
            actual.as_ref() == expected,
            "context body checksum mismatch"
        );
    }
    Ok(raw)
}
