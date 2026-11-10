use serde_json::{Value, json};

use crate::error::GatewayError;
use crate::protocol::compaction;

const FOREIGN_NOTICE: &str = "[Compaction checkpoint from a different provider was omitted because its opaque state cannot be transferred. Continue from the messages that remain visible.]";
const UNAVAILABLE_NOTICE: &str = "[Local context checkpoint was omitted because its archived session is unavailable. Continue from the messages that remain visible.]";
const UNREADABLE_NOTICE: &str = "[Local compaction summary was omitted because it could not be authenticated. Continue from the messages that remain visible.]";

/// Run after the context engine has had a chance to expand session checkpoints.
/// Only official requests can retain native opaque compaction state. Local
/// history always replays at user authority, regardless of its destination.
pub(crate) async fn prepare_compaction_replay(
    mut body: Value,
    official: bool,
) -> Result<Value, GatewayError> {
    let needs_secret = body
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| compaction_token(item).is_some_and(compaction::is_local_token))
        });
    if !needs_secret {
        normalize_compaction_items(&mut body, official, None)?;
        return Ok(body);
    }
    // Configuration locking and token decryption must not block a runtime
    // worker, and requests without local summaries never load the secret.
    tokio::task::spawn_blocking(move || {
        let secret = crate::config::ensure_compaction_secret()?;
        normalize_compaction_items(&mut body, official, Some(&secret))?;
        Ok(body)
    })
    .await
    .map_err(|error| GatewayError::Other(error.into()))?
}

fn compaction_token(item: &Value) -> Option<&str> {
    if item.get("type").and_then(Value::as_str) != Some("compaction") {
        return None;
    }
    item.get("encrypted_content").and_then(Value::as_str)
}

fn normalize_compaction_items(
    body: &mut Value,
    official: bool,
    secret: Option<&str>,
) -> Result<(), GatewayError> {
    let Some(items) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut omitted = 0;
    for item in items {
        let Some(token) = compaction_token(item) else {
            continue;
        };
        let text = if compaction::is_local_token(token) {
            let secret = secret.ok_or_else(|| {
                GatewayError::Other(anyhow::anyhow!("missing local compaction secret"))
            })?;
            match compaction::decode(token, secret) {
                Ok(summary) => {
                    format!(
                        "[Previous context summary]\n{}",
                        compaction::summary_text(&summary)
                    )
                }
                Err(GatewayError::BadRequest(_) | GatewayError::Json(_)) => {
                    omitted += 1;
                    UNREADABLE_NOTICE.to_owned()
                }
                Err(error) => return Err(error),
            }
        } else if token.starts_with(crate::context::checkpoint::CHECKPOINT_PREFIX) {
            omitted += 1;
            UNAVAILABLE_NOTICE.to_owned()
        } else if official {
            continue;
        } else {
            omitted += 1;
            FOREIGN_NOTICE.to_owned()
        };
        *item = json!({
            "type": "message", "role": "user",
            "content": [{"type": "input_text", "text": text}],
        });
    }
    if omitted > 0 {
        tracing::warn!(
            omitted,
            "replaced unavailable compaction state with explicit context notices"
        );
    }
    Ok(())
}
