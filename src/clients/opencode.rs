use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use super::files::{write_atomic_if_changed, write_owner_only};

const PROVIDER_ID: &str = "codex-mixin";
const PROVIDER_NAME: &str = "Codex Mixin";
const RESPONSES_PACKAGE: &str = "@ai-sdk/openai";

pub fn sync_client_key(key_path: &Path, client_key: &str) -> anyhow::Result<()> {
    write_owner_only(key_path, client_key.as_bytes())?;
    Ok(())
}

pub fn key_reference(key_path: &Path) -> String {
    format!("{{file:{}}}", key_path.display())
}

pub fn provider_is_managed(provider: &Value, key_path: &Path) -> bool {
    let key_reference = key_reference(key_path);
    provider.get("name").and_then(Value::as_str) == Some(PROVIDER_NAME)
        && provider.get("npm").and_then(Value::as_str) == Some(RESPONSES_PACKAGE)
        && provider.pointer("/options/apiKey").and_then(Value::as_str)
            == Some(key_reference.as_str())
}

pub fn is_managed(config_path: &Path, key_path: &Path) -> anyhow::Result<bool> {
    if !config_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(config_path)?;
    if !raw.contains(PROVIDER_NAME) {
        return Ok(false);
    }
    let document: Value = serde_json::from_str(&raw)?;
    Ok(document
        .get("provider")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .is_some_and(|provider| provider_is_managed(provider, key_path)))
}

pub fn uninstall(config_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        config_path.exists(),
        "OpenCode config is not managed by Codex Mixin: {}",
        config_path.display()
    );
    let raw = std::fs::read_to_string(config_path)?;
    let mut document: Value = if raw.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&raw)?
    };
    let root = document.as_object_mut().context(format!(
        "OpenCode config must be a JSON object: {}",
        config_path.display()
    ))?;
    let providers = root
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .context(format!(
            "OpenCode config has no provider object: {}",
            config_path.display()
        ))?;
    let provider = providers
        .get(PROVIDER_ID)
        .context("OpenCode provider codex-mixin is not installed")?;
    anyhow::ensure!(
        provider_is_managed(provider, key_path),
        "OpenCode provider {PROVIDER_ID} is not managed by Codex Mixin"
    );
    providers.remove(PROVIDER_ID);
    if providers.is_empty() {
        root.remove("provider");
    }
    let mut encoded = serde_json::to_vec_pretty(&document)?;
    encoded.push(10);
    write_atomic_if_changed(config_path, &encoded)?;
    if key_path.exists() {
        std::fs::remove_file(key_path)
            .with_context(|| format!("remove OpenCode gateway key {}", key_path.display()))?;
    }
    Ok(())
}
