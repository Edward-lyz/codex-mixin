use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};

use super::files::write_atomic_if_changed;

pub const MANAGED_MARKER: &str = "codex-mixin managed Claude Code";
const AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
const LEGACY_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];
const MANAGED_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    AUTH_TOKEN,
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT",
    "DISABLE_LOGIN_COMMAND",
];

struct SettingsBackup {
    previous_env: Map<String, Value>,
    previous_model: Option<Value>,
    managed_env_keys: Vec<String>,
    previous_model_overrides: Map<String, Value>,
    managed_model_override_keys: Vec<String>,
    previous_model_picker: Option<Value>,
    manages_model_picker: bool,
}

pub fn install(
    settings_path: &Path,
    base_url: &str,
    default_model: &str,
    model_picker: Value,
    client_key: &str,
) -> anyhow::Result<bool> {
    let raw = if settings_path.exists() {
        std::fs::read_to_string(settings_path)?
    } else {
        String::new()
    };
    let mut settings: Value = if raw.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&raw).map_err(|error| {
            anyhow::anyhow!(
                "invalid Claude Code settings {}: {error}",
                settings_path.display()
            )
        })?
    };
    let object = settings.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "Claude Code settings must be a JSON object: {}",
            settings_path.display()
        )
    })?;
    let existing_backup = object
        .get("codex_mixin_managed")
        .filter(|managed| managed.get("marker").and_then(Value::as_str) == Some(MANAGED_MARKER))
        .map(|managed| read_backup(managed, settings_path))
        .transpose()?;
    object.remove("codex_mixin_managed");

    let mut previous_env = existing_backup
        .as_ref()
        .map(|backup| backup.previous_env.clone())
        .unwrap_or_default();
    if let Some(env) = object.get_mut("env").and_then(Value::as_object_mut) {
        let env_keys = existing_backup
            .as_ref()
            .map(|backup| backup.managed_env_keys.iter().cloned().collect())
            .unwrap_or_else(BTreeSet::new)
            .into_iter()
            .chain(MANAGED_ENV_KEYS.iter().map(|key| (*key).to_owned()))
            .collect::<BTreeSet<_>>();
        for key in env_keys {
            let was_managed = existing_backup.as_ref().is_some_and(|backup| {
                backup
                    .managed_env_keys
                    .iter()
                    .any(|managed| managed == &key)
            });
            if let Some(value) = env.remove(&key)
                && !was_managed
            {
                previous_env.insert(key, value);
            }
        }
    } else if object.get("env").is_some() {
        anyhow::bail!(
            "Claude Code settings env must be a JSON object: {}",
            settings_path.display()
        );
    }
    let current_model = match object.remove("model") {
        Some(Value::Null) | None => None,
        Some(model) => Some(model),
    };
    let current_model_picker = object
        .remove("modelPicker")
        .filter(|value| !value.is_null());
    let previous_model_picker = match &existing_backup {
        Some(backup) if backup.manages_model_picker => backup.previous_model_picker.clone(),
        _ => current_model_picker,
    };
    let previous_model = match &existing_backup {
        Some(backup) => backup.previous_model.clone(),
        None => current_model,
    };
    let previous_model_overrides = existing_backup
        .as_ref()
        .map(|backup| backup.previous_model_overrides.clone())
        .unwrap_or_default();
    let remove_model_overrides = if let Some(overrides) = object.get_mut("modelOverrides") {
        let overrides = overrides.as_object_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code settings modelOverrides must be a JSON object: {}",
                settings_path.display()
            )
        })?;
        let keys = existing_backup
            .as_ref()
            .map(|backup| backup.managed_model_override_keys.iter().cloned().collect())
            .unwrap_or_else(BTreeSet::new);
        for key in keys {
            overrides.remove(&key);
        }
        overrides.is_empty()
    } else {
        false
    };
    if remove_model_overrides {
        object.remove("modelOverrides");
    }
    let env = object
        .entry("env")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code settings env must be a JSON object: {}",
                settings_path.display()
            )
        })?;
    for (key, value) in [
        ("ANTHROPIC_BASE_URL", base_url),
        (AUTH_TOKEN, client_key),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
        ("CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT", "1"),
        ("DISABLE_LOGIN_COMMAND", "1"),
    ] {
        env.insert(key.to_owned(), Value::String(value.to_owned()));
    }
    object.insert("model".to_owned(), Value::String(default_model.to_owned()));
    object.insert("modelPicker".to_owned(), model_picker);
    object.insert(
        "codex_mixin_managed".to_owned(),
        serde_json::json!({
            "marker": MANAGED_MARKER,
            "env_keys": MANAGED_ENV_KEYS,
            "model_override_keys": Vec::<String>::new(),
            "base_url": base_url,
            "model": default_model,
            "model_picker_managed": true,
            "previous_env": previous_env,
            "previous_model": previous_model,
            "previous_model_picker": previous_model_picker,
            "previous_model_overrides": previous_model_overrides
        }),
    );
    write_atomic_if_changed(settings_path, &serde_json::to_vec_pretty(&settings)?)
}

fn read_backup(managed: &Value, settings_path: &Path) -> anyhow::Result<SettingsBackup> {
    let previous_env = managed
        .get("previous_env")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code managed settings have no previous env backup: {}",
                settings_path.display()
            )
        })?;
    let previous_model_overrides = match managed.get("previous_model_overrides") {
        None => Map::new(),
        Some(Value::Object(overrides)) => overrides.clone(),
        Some(_) => anyhow::bail!(
            "Claude Code managed previous model overrides must be an object: {}",
            settings_path.display()
        ),
    };
    Ok(SettingsBackup {
        previous_env,
        previous_model: managed
            .get("previous_model")
            .filter(|value| !value.is_null())
            .cloned(),
        managed_env_keys: managed_keys(managed, "env_keys", LEGACY_ENV_KEYS, settings_path)?,
        previous_model_overrides,
        managed_model_override_keys: managed_keys(
            managed,
            "model_override_keys",
            &[],
            settings_path,
        )?,
        previous_model_picker: managed
            .get("previous_model_picker")
            .filter(|value| !value.is_null())
            .cloned(),
        manages_model_picker: managed.get("model_picker_managed").and_then(Value::as_bool)
            == Some(true),
    })
}

pub fn is_managed(settings_path: &Path) -> anyhow::Result<bool> {
    if !settings_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read(settings_path)?;
    if !String::from_utf8_lossy(&raw).contains(MANAGED_MARKER) {
        return Ok(false);
    }
    let settings: Value = serde_json::from_slice(&raw)?;
    let object = settings
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Claude Code settings must be a JSON object"))?;
    Ok(object
        .get("codex_mixin_managed")
        .and_then(|managed| managed.get("marker"))
        .and_then(Value::as_str)
        == Some(MANAGED_MARKER))
}

pub fn sync_client_key(settings_path: &Path, client_key: &str) -> anyhow::Result<()> {
    let mut settings: Value = serde_json::from_slice(&std::fs::read(settings_path)?)?;
    let object = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Claude Code settings must be a JSON object"))?;
    anyhow::ensure!(
        object
            .get("codex_mixin_managed")
            .and_then(|managed| managed.get("marker"))
            .and_then(Value::as_str)
            == Some(MANAGED_MARKER),
        "Claude Code settings are not managed by codex-mixin"
    );
    object
        .get_mut("env")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("managed Claude Code env is missing"))?
        .insert(AUTH_TOKEN.to_owned(), Value::String(client_key.to_owned()));
    write_atomic_if_changed(settings_path, &serde_json::to_vec_pretty(&settings)?)?;
    Ok(())
}

pub fn uninstall(settings_path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        settings_path.exists(),
        "Claude Code settings are not managed by codex-mixin: {}",
        settings_path.display()
    );
    let raw = std::fs::read_to_string(settings_path)?;
    let mut settings: Value = serde_json::from_str(&raw).map_err(|error| {
        anyhow::anyhow!(
            "invalid Claude Code settings {}: {error}",
            settings_path.display()
        )
    })?;
    let object = settings.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "Claude Code settings must be a JSON object: {}",
            settings_path.display()
        )
    })?;
    let managed = object.remove("codex_mixin_managed");
    anyhow::ensure!(
        managed
            .as_ref()
            .and_then(|value| value.get("marker"))
            .and_then(Value::as_str)
            == Some(MANAGED_MARKER),
        "Claude Code settings are not managed by codex-mixin: {}",
        settings_path.display()
    );
    let managed = managed
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Claude Code managed settings are missing"))?;
    let previous_env = managed
        .get("previous_env")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code managed settings have no previous env backup: {}",
                settings_path.display()
            )
        })?;
    let previous_model = managed
        .get("previous_model")
        .filter(|value| !value.is_null())
        .cloned();
    let manages_model_picker =
        managed.get("model_picker_managed").and_then(Value::as_bool) == Some(true);
    let previous_model_picker = managed
        .get("previous_model_picker")
        .filter(|value| !value.is_null())
        .cloned();
    let previous_model_overrides = match managed.get("previous_model_overrides") {
        None => Map::new(),
        Some(Value::Object(overrides)) => overrides.clone(),
        Some(_) => anyhow::bail!(
            "Claude Code managed previous model overrides must be an object: {}",
            settings_path.display()
        ),
    };
    let model_override_keys = managed_keys(managed, "model_override_keys", &[], settings_path)?;
    let env_keys = managed_keys(managed, "env_keys", LEGACY_ENV_KEYS, settings_path)?;
    if let Some(env_value) = object.get_mut("env") {
        let env = env_value.as_object_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code settings env must be a JSON object: {}",
                settings_path.display()
            )
        })?;
        for key in env_keys {
            env.remove(&key);
        }
        env.extend(previous_env);
        if env.is_empty() {
            object.remove("env");
        }
    } else if !previous_env.is_empty() {
        object.insert("env".to_owned(), Value::Object(previous_env));
    }
    let remove_overrides = if let Some(overrides) = object.get_mut("modelOverrides") {
        let overrides = overrides.as_object_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code settings modelOverrides must be a JSON object: {}",
                settings_path.display()
            )
        })?;
        for key in model_override_keys {
            overrides.remove(&key);
        }
        overrides.extend(previous_model_overrides);
        overrides.is_empty()
    } else if previous_model_overrides.is_empty() {
        false
    } else {
        object.insert(
            "modelOverrides".to_owned(),
            Value::Object(previous_model_overrides),
        );
        false
    };
    if remove_overrides {
        object.remove("modelOverrides");
    }
    match previous_model {
        Some(previous) => {
            object.insert("model".to_owned(), previous);
        }
        None => {
            object.remove("model");
        }
    }
    if manages_model_picker {
        match previous_model_picker {
            Some(previous) => {
                object.insert("modelPicker".to_owned(), previous);
            }
            None => {
                object.remove("modelPicker");
            }
        }
    }
    write_atomic_if_changed(settings_path, &serde_json::to_vec_pretty(&settings)?)?;
    Ok(())
}

fn managed_keys(
    managed: &Value,
    field: &str,
    fallback: &[&str],
    settings_path: &Path,
) -> anyhow::Result<Vec<String>> {
    match managed.get(field) {
        None => Ok(fallback.iter().map(|key| (*key).to_owned()).collect()),
        Some(Value::Array(keys)) => keys
            .iter()
            .map(|key| {
                key.as_str().map(str::to_owned).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Claude Code managed {field} entry must be a string: {}",
                        settings_path.display()
                    )
                })
            })
            .collect(),
        Some(_) => anyhow::bail!(
            "Claude Code managed {field} must be an array: {}",
            settings_path.display()
        ),
    }
}
