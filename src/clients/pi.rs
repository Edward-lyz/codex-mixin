use std::net::SocketAddr;
use std::path::Path;

use anyhow::Context;
use serde_json::{Map, Value, json};

use super::files::{set_owner_only, write_atomic_if_changed, write_owner_only};

const PROVIDER_ID: &str = "codex-mixin";
const PROVIDER_NAME: &str = "Codex Mixin";
const API: &str = "openai-responses";

pub fn install(
    models_path: &Path,
    key_path: &Path,
    bind: SocketAddr,
    models: Vec<Value>,
    client_key: &str,
) -> anyhow::Result<bool> {
    let mut document = read_models(models_path)?;
    let root = document.as_object_mut().context(format!(
        "Pi models config must be a JSON object: {}",
        models_path.display()
    ))?;
    let providers = root
        .entry("providers".to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context(format!(
            "Pi models config providers must be a JSON object: {}",
            models_path.display()
        ))?;
    if let Some(existing) = providers.get(PROVIDER_ID) {
        anyhow::ensure!(
            provider_is_managed(existing, key_path),
            "Pi provider {PROVIDER_ID} already exists and is not managed by Codex Mixin"
        );
    }
    providers.insert(
        PROVIDER_ID.to_owned(),
        json!({
            "name": PROVIDER_NAME,
            "baseUrl": format!("http://{bind}/v1"),
            "apiKey": key_reference(key_path),
            "api": API,
            "models": models,
        }),
    );
    sync_client_key(key_path, client_key)?;
    write_json(models_path, &document)
}

pub fn sync_client_key(key_path: &Path, client_key: &str) -> anyhow::Result<()> {
    write_owner_only(key_path, client_key.as_bytes())?;
    Ok(())
}

pub fn key_reference(key_path: &Path) -> String {
    let escaped_path = key_path.to_string_lossy().replace(char::from(39), "'\\''");
    format!("!cat '{escaped_path}'")
}

pub fn provider_is_managed(provider: &Value, key_path: &Path) -> bool {
    let key_reference = key_reference(key_path);
    provider.get("name").and_then(Value::as_str) == Some(PROVIDER_NAME)
        && provider.get("api").and_then(Value::as_str) == Some(API)
        && provider.get("apiKey").and_then(Value::as_str) == Some(key_reference.as_str())
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
    Ok(document
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .is_some_and(|provider| provider_is_managed(provider, key_path)))
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
    let provider = providers
        .get(PROVIDER_ID)
        .context("Pi provider codex-mixin is not installed")?;
    anyhow::ensure!(
        provider_is_managed(provider, key_path),
        "Pi provider {PROVIDER_ID} is not managed by Codex Mixin"
    );
    providers.remove(PROVIDER_ID);
    if providers.is_empty() {
        root.remove("providers");
    }
    write_json(models_path, &document)?;
    if key_path.exists() {
        std::fs::remove_file(key_path)
            .with_context(|| format!("remove Pi gateway key {}", key_path.display()))?;
    }
    Ok(())
}

fn read_models(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(json!({"providers": {}}));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read Pi models config {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(json!({"providers": {}}));
    }
    serde_json::from_str(&raw).with_context(|| format!("parse Pi models config {}", path.display()))
}

fn write_json(path: &Path, document: &Value) -> anyhow::Result<bool> {
    let existed = path.exists();
    let mut encoded = serde_json::to_vec_pretty(document)?;
    encoded.push(10);
    let changed = write_atomic_if_changed(path, &encoded)?;
    if !existed {
        set_owner_only(path)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_reference_shell_quotes_apostrophes() {
        assert_eq!(
            key_reference(Path::new("/tmp/pi's key")),
            "!cat '/tmp/pi'\\''s key'"
        );
    }
}
