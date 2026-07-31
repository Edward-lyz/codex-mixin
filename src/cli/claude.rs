use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::runtime::{load_runtime_metadata, pid_is_running};
use codex_mixin::config::GatewayConfig;
use codex_mixin::provider::ProviderProtocol;

pub(in crate::cli) const MANAGED_CLAUDE_MARKER: &str = "codex-mixin managed Claude Code";
const MANAGED_CLAUDE_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];

pub(in crate::cli) fn default_claude_settings_path() -> PathBuf {
    std::env::var("HOME")
        .map(|home| PathBuf::from(home).join(".claude").join("settings.json"))
        .unwrap_or_else(|_| PathBuf::from(".claude/settings.json"))
}

pub(in crate::cli) fn resolve_claude_settings_path(
    settings_path: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    std::path::absolute(settings_path.unwrap_or_else(default_claude_settings_path))
        .map_err(Into::into)
}

pub(in crate::cli) fn install_claude(
    settings_path: Option<PathBuf>,
    requested_model: Option<String>,
) -> anyhow::Result<()> {
    let settings_path = resolve_claude_settings_path(settings_path)?;
    let gateway_config = GatewayConfig::from_stored_config()?;
    let model = select_claude_model(&gateway_config, requested_model.as_deref())?;
    let gateway_bind = match load_runtime_metadata()? {
        Some(runtime) if pid_is_running(runtime.pid)? => runtime.bind,
        _ => gateway_config.bind,
    };
    let base_url = format!("http://{gateway_bind}");
    let raw = if settings_path.exists() {
        fs::read_to_string(&settings_path)?
    } else {
        String::new()
    };
    let mut settings: Value = if raw.trim().is_empty() {
        json!({})
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
    let mut previous_env = serde_json::Map::new();
    if let Some(env) = object.get_mut("env").and_then(Value::as_object_mut) {
        for key in MANAGED_CLAUDE_ENV_KEYS {
            if let Some(value) = env.remove(*key) {
                previous_env.insert((*key).to_owned(), value);
            }
        }
    } else if object.get("env").is_some() {
        anyhow::bail!(
            "Claude Code settings env must be a JSON object: {}",
            settings_path.display()
        );
    }
    let previous_model = match object.remove("model") {
        Some(Value::Null) | None => None,
        Some(model) => Some(model),
    };
    let env = object
        .entry("env")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code settings env must be a JSON object: {}",
                settings_path.display()
            )
        })?;
    env.insert(
        "ANTHROPIC_BASE_URL".to_owned(),
        Value::String(base_url.clone()),
    );
    env.insert("ANTHROPIC_MODEL".to_owned(), Value::String(model.clone()));
    env.insert(
        "ANTHROPIC_DEFAULT_SONNET_MODEL".to_owned(),
        Value::String(model.clone()),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_OPUS_MODEL".to_owned(),
        Value::String(model.clone()),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_owned(),
        Value::String(model.clone()),
    );
    object.insert("model".to_owned(), Value::String(model.clone()));
    object.insert(
        "codex_mixin_managed".to_owned(),
        json!({
            "marker": MANAGED_CLAUDE_MARKER,
            "base_url": base_url,
            "model": model,
            "previous_env": previous_env,
            "previous_model": previous_model
        }),
    );
    write_atomic_if_changed(&settings_path, &serde_json::to_vec_pretty(&settings)?)?;
    println!("claude code settings updated: {}", settings_path.display());
    println!("ANTHROPIC_BASE_URL: {base_url}");
    println!("claude code model: {model}");
    println!("reload required: restart Claude Code or start a new session");
    Ok(())
}

pub(in crate::cli) fn uninstall_claude(settings_path: Option<PathBuf>) -> anyhow::Result<()> {
    let settings_path = resolve_claude_settings_path(settings_path)?;
    if !settings_path.exists() {
        anyhow::bail!(
            "Claude Code settings are not managed by codex-mixin: {}",
            settings_path.display()
        );
    }
    let raw = fs::read_to_string(&settings_path)?;
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
    if managed
        .as_ref()
        .and_then(|value| value.get("marker"))
        .and_then(Value::as_str)
        != Some(MANAGED_CLAUDE_MARKER)
    {
        anyhow::bail!(
            "Claude Code settings are not managed by codex-mixin: {}",
            settings_path.display()
        );
    }
    let previous_env = managed
        .as_ref()
        .and_then(|value| value.get("previous_env"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let previous_model = managed
        .as_ref()
        .and_then(|value| value.get("previous_model"))
        .filter(|value| !value.is_null())
        .cloned();
    if let Some(env) = object.get_mut("env").and_then(Value::as_object_mut) {
        for key in MANAGED_CLAUDE_ENV_KEYS {
            env.remove(*key);
        }
        env.extend(previous_env);
        if env.is_empty() {
            object.remove("env");
        }
    }
    match previous_model {
        Some(previous_model) => {
            object.insert("model".to_owned(), previous_model);
        }
        None => {
            object.remove("model");
        }
    }
    write_atomic_if_changed(&settings_path, &serde_json::to_vec_pretty(&settings)?)?;
    println!("claude code settings restored: {}", settings_path.display());
    println!("ANTHROPIC_BASE_URL removed; restart Claude Code to apply");
    Ok(())
}

pub(in crate::cli) fn claude_status(settings_path: Option<PathBuf>) -> anyhow::Result<()> {
    let settings_path = resolve_claude_settings_path(settings_path)?;
    if !settings_path.exists() {
        println!("claude-code: not installed");
        return Ok(());
    }
    let raw = fs::read_to_string(&settings_path)?;
    let settings: Value = serde_json::from_str(&raw).map_err(|error| {
        anyhow::anyhow!(
            "invalid Claude Code settings {}: {error}",
            settings_path.display()
        )
    })?;
    if settings
        .get("codex_mixin_managed")
        .and_then(|value| value.get("marker"))
        .and_then(Value::as_str)
        == Some(MANAGED_CLAUDE_MARKER)
    {
        println!(
            "claude-code: installed via {}",
            settings_path.display()
        );
    } else {
        println!("claude-code: not installed");
    }
    Ok(())
}

fn select_claude_model(
    config: &GatewayConfig,
    requested_model: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(requested_model) = requested_model {
        let exists = config.providers.iter().any(|provider| {
            provider.enabled
                && provider
                    .selected_models
                    .iter()
                    .any(|model| model.eq_ignore_ascii_case(requested_model))
        });
        if !exists {
            anyhow::bail!("requested Claude model is not configured: {requested_model}");
        }
        return Ok(requested_model.to_owned());
    }
    let mut best: Option<(usize, String)> = None;
    for provider in &config.providers {
        if !provider.enabled || provider.protocol != ProviderProtocol::AnthropicMessages {
            continue;
        }
        for model in &provider.selected_models {
            let lower = model.to_ascii_lowercase();
            let score = if lower.contains("claude") {
                4
            } else if lower.contains("opus") {
                3
            } else if lower.contains("haiku") {
                2
            } else {
                1
            };
            if best.as_ref().is_none_or(|(current, _)| score > *current) {
                best = Some((score, model.clone()));
            }
        }
    }
    best.map(|(_, model)| model)
        .ok_or_else(|| anyhow::anyhow!("no enabled Anthropic Messages model is configured"))
}
