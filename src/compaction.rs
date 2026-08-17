use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::ensure_compaction_secret;
use crate::error::GatewayError;

pub(crate) const TOKEN_PREFIX: &str = "codex-mixin:compaction:v1:";
pub(crate) const MAX_SUMMARY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactionSummary {
    pub goal: String,
    pub constraints: Vec<String>,
    pub decisions: Vec<String>,
    pub files: Vec<String>,
    pub tool_results: Vec<String>,
    pub pending_work: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CompactionPayload {
    version: u8,
    model: String,
    created_at: u64,
    summary: CompactionSummary,
}

pub(crate) fn encode(model: &str, summary: CompactionSummary) -> Result<String, GatewayError> {
    let secret = ensure_compaction_secret().map_err(GatewayError::Other)?;
    encode_with_secret(model, summary, &secret)
}

fn encode_with_secret(
    model: &str,
    summary: CompactionSummary,
    secret: &str,
) -> Result<String, GatewayError> {
    let payload = CompactionPayload {
        version: 1,
        model: model.to_owned(),
        created_at: unix_seconds()?,
        summary,
    };
    let plaintext = serde_json::to_vec(&payload)?;
    if plaintext.len() > MAX_SUMMARY_BYTES {
        return Err(GatewayError::BadRequest(
            "compaction summary exceeds 64 KiB".to_owned(),
        ));
    }
    let key = key_from_secret(secret)?;
    let mut encrypted = plaintext;
    let mut nonce_bytes = [0_u8; 12];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| GatewayError::Other(anyhow::anyhow!("generate compaction nonce")))?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    key.seal_in_place_append_tag(nonce, Aad::empty(), &mut encrypted)
        .map_err(|_| GatewayError::Other(anyhow::anyhow!("encrypt compaction summary")))?;
    let mut token = nonce_bytes.to_vec();
    token.extend(encrypted);
    Ok(format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(token)))
}

pub(crate) fn decode(token: &str, expected_model: &str) -> Result<CompactionSummary, GatewayError> {
    let secret = ensure_compaction_secret().map_err(GatewayError::Other)?;
    decode_with_secret(token, expected_model, &secret)
}

fn decode_with_secret(
    token: &str,
    expected_model: &str,
    secret: &str,
) -> Result<CompactionSummary, GatewayError> {
    let encoded = token
        .strip_prefix(TOKEN_PREFIX)
        .ok_or_else(|| GatewayError::BadRequest("unsupported compaction token".to_owned()))?;
    let mut token = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| GatewayError::BadRequest("invalid compaction token".to_owned()))?;
    if token.len() < 12 {
        return Err(GatewayError::BadRequest(
            "invalid compaction token".to_owned(),
        ));
    }
    let nonce_bytes: [u8; 12] = token[..12]
        .try_into()
        .map_err(|_| GatewayError::BadRequest("invalid compaction nonce".to_owned()))?;
    let ciphertext = token.split_off(12);
    let mut sealed = ciphertext;
    let key = key_from_secret(secret)?;
    let plaintext = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::empty(),
            &mut sealed,
        )
        .map_err(|_| GatewayError::BadRequest("invalid compaction token".to_owned()))?;
    let payload: CompactionPayload = serde_json::from_slice(plaintext)?;
    if payload.version != 1 || payload.model != expected_model {
        return Err(GatewayError::BadRequest(
            "compaction token model or version mismatch".to_owned(),
        ));
    }
    let summary = serde_json::to_vec(&payload.summary)?;
    if summary.len() > MAX_SUMMARY_BYTES {
        return Err(GatewayError::BadRequest(
            "compaction summary exceeds 64 KiB".to_owned(),
        ));
    }
    Ok(payload.summary)
}

pub(crate) fn summary_text(summary: &CompactionSummary) -> String {
    format!(
        "[Conversation summary from codex-mixin compaction]\nGoal: {}\nConstraints: {}\nDecisions: {}\nFiles: {}\nTool results: {}\nPending work: {}\n[End conversation summary]",
        summary.goal,
        summary.constraints.join("\n- "),
        summary.decisions.join("\n- "),
        summary.files.join("\n- "),
        summary.tool_results.join("\n- "),
        summary.pending_work.join("\n- "),
    )
}

fn key_from_secret(secret: &str) -> Result<LessSafeKey, GatewayError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(secret)
        .map_err(|_| GatewayError::Other(anyhow::anyhow!("invalid compaction secret")))?;
    let key = UnboundKey::new(&AES_256_GCM, &bytes)
        .map_err(|_| GatewayError::Other(anyhow::anyhow!("invalid compaction secret")))?;
    Ok(LessSafeKey::new(key))
}

fn unix_seconds() -> Result<u64, GatewayError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| GatewayError::Other(error.into()))?
        .as_secs())
}

pub(crate) fn summary_from_value(value: Value) -> Result<CompactionSummary, GatewayError> {
    let summary: CompactionSummary = serde_json::from_value(value).map_err(|error| {
        GatewayError::BadRequest(format!("invalid compaction summary: {error}"))
    })?;
    if summary.goal.trim().is_empty() {
        return Err(GatewayError::BadRequest(
            "compaction summary goal must not be empty".to_owned(),
        ));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> CompactionSummary {
        CompactionSummary {
            goal: "ship compact".to_owned(),
            constraints: vec!["no tools".to_owned()],
            decisions: vec!["same model".to_owned()],
            files: vec!["src/server/compact.rs".to_owned()],
            tool_results: vec!["tests passed".to_owned()],
            pending_work: vec!["add E2E".to_owned()],
        }
    }

    #[test]
    fn round_trips_and_binds_model() {
        let secret = URL_SAFE_NO_PAD.encode([7_u8; 32]);
        let token = encode_with_secret("model-a", summary(), &secret).unwrap();
        let decoded = decode_with_secret(&token, "model-a", &secret).unwrap();
        assert_eq!(decoded.goal, "ship compact");
        assert!(decode_with_secret(&token, "model-b", &secret).is_err());
    }

    #[test]
    fn rejects_tampered_tokens_and_unknown_summary_fields() {
        let secret = URL_SAFE_NO_PAD.encode([9_u8; 32]);
        let token = encode_with_secret("model-a", summary(), &secret).unwrap();
        let mut tampered = token.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert!(
            decode_with_secret(std::str::from_utf8(&tampered).unwrap(), "model-a", &secret)
                .is_err()
        );
        assert!(
            summary_from_value(serde_json::json!({
                "goal": "x",
                "constraints": [],
                "decisions": [],
                "files": [],
                "tool_results": [],
                "pending_work": [],
                "unknown": true
            }))
            .is_err()
        );
    }
}
