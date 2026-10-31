//! Authenticated local compaction checkpoints.
//!
//! A checkpoint replaces the history the client gave up. It is bound to one
//! context session and to the revision the engine had when the checkpoint was
//! issued, so a token cannot be replayed against another session, and a token
//! from a newer engine state fails instead of silently losing messages.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::GatewayError;

pub(crate) const CHECKPOINT_PREFIX: &str = "codex-mixin:context:v2:";
const MAX_PAYLOAD_BYTES: usize = 8 * 1024;
const VERSION: u8 = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Checkpoint {
    version: u8,
    pub model: String,
    pub created_at: u64,
    pub session: String,
    pub revision: i64,
}

/// Returns true when the item carries a local checkpoint token.
pub(crate) fn is_checkpoint_item(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("compaction")
        && item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(|token| token.starts_with(CHECKPOINT_PREFIX))
}

/// Issues a checkpoint for the given session and revision.
pub(crate) fn encode(
    model: &str,
    session: &str,
    revision: i64,
    secret: &str,
) -> Result<String, GatewayError> {
    let payload = Checkpoint {
        version: VERSION,
        model: model.to_owned(),
        created_at: unix_seconds()?,
        session: session.to_owned(),
        revision,
    };
    let plaintext = serde_json::to_vec(&payload)?;
    if plaintext.len() > MAX_PAYLOAD_BYTES {
        return Err(GatewayError::BadRequest(
            "context checkpoint exceeds 8 KiB".to_owned(),
        ));
    }
    seal(&plaintext, secret)
}

/// Opens a checkpoint token, rejecting tampering and other versions.
pub(crate) fn decode(token: &str, secret: &str) -> Result<Checkpoint, GatewayError> {
    let encoded = token
        .strip_prefix(CHECKPOINT_PREFIX)
        .ok_or_else(|| GatewayError::BadRequest("unsupported context checkpoint".to_owned()))?;
    let plaintext = open(encoded, secret)?;
    let checkpoint: Checkpoint = serde_json::from_slice(&plaintext)?;
    if checkpoint.version != VERSION {
        return Err(GatewayError::BadRequest(
            "context checkpoint version mismatch".to_owned(),
        ));
    }
    if checkpoint.session.is_empty() {
        return Err(GatewayError::BadRequest(
            "context checkpoint has no session".to_owned(),
        ));
    }
    Ok(checkpoint)
}

fn seal(plaintext: &[u8], secret: &str) -> Result<String, GatewayError> {
    let key = key_from_secret(secret)?;
    let mut encrypted = plaintext.to_vec();
    let mut nonce_bytes = [0_u8; 12];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| GatewayError::Other(anyhow::anyhow!("generate context checkpoint nonce")))?;
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce_bytes),
        Aad::empty(),
        &mut encrypted,
    )
    .map_err(|_| GatewayError::Other(anyhow::anyhow!("encrypt context checkpoint")))?;
    let mut token = nonce_bytes.to_vec();
    token.extend(encrypted);
    Ok(format!(
        "{CHECKPOINT_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(token)
    ))
}

fn open(encoded: &str, secret: &str) -> Result<Vec<u8>, GatewayError> {
    let mut token = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| GatewayError::BadRequest("invalid context checkpoint".to_owned()))?;
    if token.len() < 12 {
        return Err(GatewayError::BadRequest(
            "invalid context checkpoint".to_owned(),
        ));
    }
    let nonce_bytes: [u8; 12] = token[..12]
        .try_into()
        .map_err(|_| GatewayError::BadRequest("invalid context checkpoint".to_owned()))?;
    let mut sealed = token.split_off(12);
    let key = key_from_secret(secret)?;
    let plaintext = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::empty(),
            &mut sealed,
        )
        .map_err(|_| GatewayError::BadRequest("invalid context checkpoint".to_owned()))?;
    Ok(plaintext.to_vec())
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
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| GatewayError::Other(error.into()))?
        .as_secs())
}
