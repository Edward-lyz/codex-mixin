use std::path::Path;

use anyhow::Context;
use serde_yaml::{Mapping, Value};

use super::files::{set_owner_only, write_atomic_if_changed};

const PROVIDER_ID: &str = "codex-mixin";
const API_KEY_ENV: &str = "CODEX_MIXIN_GATEWAY_API_KEY";

pub fn is_managed(dsh_home: &Path) -> anyhow::Result<bool> {
    let settings_path = dsh_home.join("settings.yaml");
    if !settings_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(&settings_path)?;
    if !raw.contains("displayName: Codex Mixin") {
        return Ok(false);
    }
    let settings = read_yaml(&settings_path, "DSH settings")?;
    Ok(settings
        .get("llm-pi-ai")
        .and_then(|value| value.get("providers"))
        .and_then(|value| value.get(PROVIDER_ID))
        .and_then(|provider| provider.get("displayName"))
        .and_then(Value::as_str)
        == Some("Codex Mixin"))
}

pub fn sync_client_key(dsh_home: &Path, client_key: &str) -> anyhow::Result<()> {
    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
    credentials
        .as_mapping_mut()
        .context("DSH credentials must be a YAML mapping")?
        .insert(
            Value::String(API_KEY_ENV.to_owned()),
            Value::String(client_key.to_owned()),
        );
    let contents = serde_yaml::to_string(&credentials)
        .with_context(|| format!("serialize DSH YAML {}", credentials_path.display()))?;
    write_atomic_if_changed(&credentials_path, contents.as_bytes())?;
    set_owner_only(&credentials_path)
}

fn read_yaml(path: &Path, label: &str) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(Value::Mapping(Mapping::new()));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read {label} {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Value::Mapping(Mapping::new()));
    }
    serde_yaml::from_str(&raw).with_context(|| format!("parse {label} {}", path.display()))
}
