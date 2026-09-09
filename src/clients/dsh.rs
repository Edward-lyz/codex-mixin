use std::net::SocketAddr;
use std::path::Path;

use anyhow::Context;
use serde_yaml::{Mapping, Value};

use super::files::{ensure_owner_only_dir, set_owner_only, write_atomic_if_changed};

const PROVIDER_ID: &str = "codex-mixin";
const API_KEY_ENV: &str = "CODEX_MIXIN_GATEWAY_API_KEY";
const API_PROTOCOL: &str = "openai-responses";

pub fn install(
    dsh_home: &Path,
    bind: SocketAddr,
    models: Vec<Value>,
    client_key: &str,
) -> anyhow::Result<bool> {
    ensure_owner_only_dir(dsh_home)?;
    let settings_path = dsh_home.join("settings.yaml");
    let mut settings = read_yaml(&settings_path, "DSH settings")?;
    let root = settings
        .as_mapping_mut()
        .context("DSH settings must be a YAML mapping")?;
    let llm_pi_ai = root
        .entry(Value::String("llm-pi-ai".to_owned()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .context("DSH settings llm-pi-ai must be a YAML mapping")?;
    let providers = llm_pi_ai
        .entry(Value::String("providers".to_owned()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .context("DSH settings llm-pi-ai.providers must be a YAML mapping")?;
    providers.insert(
        Value::String(PROVIDER_ID.to_owned()),
        provider_profile(bind, models),
    );

    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
    credentials
        .as_mapping_mut()
        .context("DSH credentials must be a YAML mapping")?
        .insert(
            Value::String(API_KEY_ENV.to_owned()),
            Value::String(client_key.to_owned()),
        );
    let credentials_changed = write_yaml(&credentials_path, &credentials)?;
    let settings_changed = write_yaml(&settings_path, &settings)?;
    Ok(credentials_changed || settings_changed)
}

pub fn uninstall(dsh_home: &Path) -> anyhow::Result<()> {
    let settings_path = dsh_home.join("settings.yaml");
    anyhow::ensure!(
        settings_path.exists(),
        "DSH settings are not managed by codex-mixin: {}",
        settings_path.display()
    );
    let mut settings = read_yaml(&settings_path, "DSH settings")?;
    let root = settings
        .as_mapping_mut()
        .context("DSH settings must be a YAML mapping")?;
    let llm_pi_ai = root
        .get_mut(Value::String("llm-pi-ai".to_owned()))
        .and_then(Value::as_mapping_mut)
        .context("DSH settings llm-pi-ai must be a YAML mapping")?;
    let providers = llm_pi_ai
        .get_mut(Value::String("providers".to_owned()))
        .and_then(Value::as_mapping_mut)
        .context("DSH settings llm-pi-ai.providers must be a YAML mapping")?;
    let profile = providers
        .remove(Value::String(PROVIDER_ID.to_owned()))
        .context("DSH provider codex-mixin is not installed")?;

    if profile.get("apiKeyEnv").and_then(Value::as_str) == Some(API_KEY_ENV) {
        let credentials_path = dsh_home.join(".credentials.yaml");
        if credentials_path.exists() {
            let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
            credentials
                .as_mapping_mut()
                .context("DSH credentials must be a YAML mapping")?
                .remove(Value::String(API_KEY_ENV.to_owned()));
            write_yaml(&credentials_path, &credentials)?;
        }
    }
    if providers.is_empty() {
        llm_pi_ai.remove(Value::String("providers".to_owned()));
    }
    if llm_pi_ai.is_empty() {
        root.remove(Value::String("llm-pi-ai".to_owned()));
    }
    if root.is_empty() {
        std::fs::remove_file(&settings_path)
            .with_context(|| format!("remove empty DSH settings {}", settings_path.display()))?;
    } else {
        write_yaml(&settings_path, &settings)?;
    }
    Ok(())
}

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

fn write_yaml(path: &Path, value: &Value) -> anyhow::Result<bool> {
    let contents = serde_yaml::to_string(value)
        .with_context(|| format!("serialize DSH YAML {}", path.display()))?;
    let changed = write_atomic_if_changed(path, contents.as_bytes())?;
    set_owner_only(path)?;
    Ok(changed)
}

fn provider_profile(bind: SocketAddr, models: Vec<Value>) -> Value {
    let mut profile = Mapping::new();
    profile.insert(
        Value::String("api".to_owned()),
        Value::String(API_PROTOCOL.to_owned()),
    );
    profile.insert(
        Value::String("displayName".to_owned()),
        Value::String("Codex Mixin".to_owned()),
    );
    profile.insert(
        Value::String("baseURL".to_owned()),
        Value::String(format!("http://{bind}/v1")),
    );
    profile.insert(
        Value::String("apiKeyEnv".to_owned()),
        Value::String(API_KEY_ENV.to_owned()),
    );
    profile.insert(Value::String("models".to_owned()), Value::Sequence(models));
    Value::Mapping(profile)
}
