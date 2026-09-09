use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use super::files::{write_atomic_if_changed, write_owner_only};

const PROVIDER_ID: &str = "codex-mixin";
const PROVIDER_NAME: &str = "Codex Mixin";
const API: &str = "openai-responses";

pub fn sync_client_key(key_path: &Path, client_key: &str) -> anyhow::Result<()> {
    write_owner_only(key_path, client_key.as_bytes())?;
    Ok(())
}

pub fn is_managed(models_path: &Path, key_path: &Path) -> anyhow::Result<bool> {
    if !models_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(models_path)?;
    let document: Value = if raw.trim().is_empty() {
        serde_json::json!({"providers": {}})
    } else {
        serde_json::from_str(&raw)?
    };
    let escaped_path = key_path.to_string_lossy().replace(char::from(39), "'\''");
    let key_reference = format!("!cat '{escaped_path}'");
    Ok(document
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .is_some_and(|provider| {
            provider.get("name").and_then(Value::as_str) == Some(PROVIDER_NAME)
                && provider.get("api").and_then(Value::as_str) == Some(API)
                && provider.get("apiKey").and_then(Value::as_str) == Some(key_reference.as_str())
        }))
}

pub fn uninstall(models_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        models_path.exists(),
        "Pi models config is not managed by Codex Mixin: {}",
        models_path.display()
    );
    let raw = std::fs::read_to_string(models_path)?;
    let mut document: Value = if raw.trim().is_empty() {
        serde_json::json!({"providers": {}})
    } else {
        serde_json::from_str(&raw)?
    };
    let root = document.as_object_mut().context(format!(
        "Pi models config must be a JSON object: {}",
        models_path.display()
    ))?;
    let providers = root
        .get_mut("providers")
        .and_then(Value::as_object_mut)
        .context(format!(
            "Pi models config has no providers object: {}",
            models_path.display()
        ))?;
    let escaped_path = key_path.to_string_lossy().replace(char::from(39), "'\''");
    let key_reference = format!("!cat '{escaped_path}'");
    let provider = providers
        .get(PROVIDER_ID)
        .context("Pi provider codex-mixin is not installed")?;
    anyhow::ensure!(
        provider.get("name").and_then(Value::as_str) == Some(PROVIDER_NAME)
            && provider.get("api").and_then(Value::as_str) == Some(API)
            && provider.get("apiKey").and_then(Value::as_str) == Some(key_reference.as_str()),
        "Pi provider {PROVIDER_ID} is not managed by Codex Mixin"
    );
    providers.remove(PROVIDER_ID);
    if providers.is_empty() {
        root.remove("providers");
    }
    let mut encoded = serde_json::to_vec_pretty(&document)?;
    encoded.push(10);
    write_atomic_if_changed(models_path, &encoded)?;
    if key_path.exists() {
        std::fs::remove_file(key_path)
            .with_context(|| format!("remove Pi gateway key {}", key_path.display()))?;
    }
    Ok(())
}
