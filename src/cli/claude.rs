use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::report_hook::reporting_enabled;
use crate::cli::runtime::{load_runtime_metadata, pid_is_running};
use codex_mixin::config::GatewayConfig;
use codex_mixin::provider::{ProviderModel, ProviderRegistry};

use super::official_models::selected_official_models;

pub(in crate::cli) const MANAGED_CLAUDE_MARKER: &str = "codex-mixin managed Claude Code";
const MANAGED_CLAUDE_HOOK_MARKER: &str = " report-hook --event ";
const CLAUDE_DEFAULT_MODEL: &str = "sonnet";
const CLAUDE_LOCAL_AUTH_TOKEN: &str = "codex-mixin-local";
const LEGACY_MANAGED_CLAUDE_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];
const MANAGED_CLAUDE_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "DISABLE_LOGIN_COMMAND",
];
const CLAUDE_OPUS_OVERRIDE_ID: &str = "claude-opus-4-6";
const CLAUDE_SONNET_OVERRIDE_ID: &str = "claude-sonnet-4-6";
const CLAUDE_HAIKU_OVERRIDE_ID: &str = "claude-haiku-4-5-20251001";
const MANAGED_CLAUDE_MODEL_OVERRIDE_KEYS: &[&str] = &[
    CLAUDE_OPUS_OVERRIDE_ID,
    CLAUDE_SONNET_OVERRIDE_ID,
    CLAUDE_HAIKU_OVERRIDE_ID,
];

struct ClaudeSettingsBackup {
    previous_env: Map<String, Value>,
    previous_model: Option<Value>,
    managed_env_keys: Vec<String>,
    previous_model_overrides: Map<String, Value>,
    managed_model_override_keys: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::cli) struct ClaudeModelMapping {
    pub(in crate::cli) opus: String,
    pub(in crate::cli) sonnet: String,
    pub(in crate::cli) haiku: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::cli) enum ClaudeModelRequest {
    Automatic,
    Single(String),
    Mapping(ClaudeModelMapping),
}

pub(in crate::cli) fn claude_model_request(
    model: Option<String>,
    opus_model: Option<String>,
    sonnet_model: Option<String>,
    haiku_model: Option<String>,
) -> anyhow::Result<ClaudeModelRequest> {
    match (model, opus_model, sonnet_model, haiku_model) {
        (None, None, None, None) => Ok(ClaudeModelRequest::Automatic),
        (Some(model), None, None, None) => Ok(ClaudeModelRequest::Single(model)),
        (None, Some(opus), Some(sonnet), Some(haiku)) => {
            Ok(ClaudeModelRequest::Mapping(ClaudeModelMapping {
                opus,
                sonnet,
                haiku,
            }))
        }
        _ => anyhow::bail!(
            "set --model alone, or set --opus-model, --sonnet-model, and --haiku-model together"
        ),
    }
}

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

fn managed_claude_keys(
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

pub(in crate::cli) fn install_claude(
    settings_path: Option<PathBuf>,
    model_request: ClaudeModelRequest,
) -> anyhow::Result<()> {
    let gateway_config = GatewayConfig::from_stored_config()?;
    let official_models = selected_official_models(&gateway_config)?;
    install_claude_with_models(
        settings_path,
        model_request,
        &gateway_config,
        &official_models,
    )
}

#[cfg(test)]
pub(in crate::cli) fn install_claude_with_config(
    settings_path: Option<PathBuf>,
    model_request: ClaudeModelRequest,
    gateway_config: &GatewayConfig,
) -> anyhow::Result<()> {
    install_claude_with_models(settings_path, model_request, gateway_config, &[])
}

fn install_claude_with_models(
    settings_path: Option<PathBuf>,
    model_request: ClaudeModelRequest,
    gateway_config: &GatewayConfig,
    official_models: &[ProviderModel],
) -> anyhow::Result<()> {
    let settings_path = resolve_claude_settings_path(settings_path)?;
    let models = resolve_claude_models(gateway_config, official_models, model_request)?;
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
    let existing_backup = object
        .get("codex_mixin_managed")
        .filter(|managed| {
            managed.get("marker").and_then(Value::as_str) == Some(MANAGED_CLAUDE_MARKER)
        })
        .map(|managed| {
            let previous_env = managed
                .get("previous_env")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Claude Code managed settings have no previous env backup: {}",
                        settings_path.display()
                    )
                })?
                .clone();
            let previous_model = managed
                .get("previous_model")
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
            Ok::<_, anyhow::Error>(ClaudeSettingsBackup {
                previous_env,
                previous_model,
                managed_env_keys: managed_claude_keys(
                    managed,
                    "env_keys",
                    LEGACY_MANAGED_CLAUDE_ENV_KEYS,
                    &settings_path,
                )?,
                previous_model_overrides,
                managed_model_override_keys: managed_claude_keys(
                    managed,
                    "model_override_keys",
                    &[],
                    &settings_path,
                )?,
            })
        })
        .transpose()?;
    object.remove("codex_mixin_managed");

    let mut previous_env = existing_backup
        .as_ref()
        .map(|backup| backup.previous_env.clone())
        .unwrap_or_default();
    if let Some(env) = object.get_mut("env").and_then(Value::as_object_mut) {
        for key in MANAGED_CLAUDE_ENV_KEYS {
            let was_managed = existing_backup.as_ref().is_some_and(|backup| {
                backup
                    .managed_env_keys
                    .iter()
                    .any(|managed_key| managed_key == key)
            });
            if let Some(value) = env.remove(*key)
                && !was_managed
            {
                previous_env.insert((*key).to_owned(), value);
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
    let previous_model = match &existing_backup {
        Some(backup) => backup.previous_model.clone(),
        None => current_model,
    };
    let mut previous_model_overrides = existing_backup
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
        for key in MANAGED_CLAUDE_MODEL_OVERRIDE_KEYS {
            let was_managed = existing_backup.as_ref().is_some_and(|backup| {
                backup
                    .managed_model_override_keys
                    .iter()
                    .any(|managed_key| managed_key == key)
            });
            if let Some(value) = overrides.remove(*key)
                && !was_managed
            {
                previous_model_overrides.insert((*key).to_owned(), value);
            }
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
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".to_owned(),
        Value::String(
            gateway_config
                .gateway_api_key
                .clone()
                .unwrap_or_else(|| CLAUDE_LOCAL_AUTH_TOKEN.to_owned()),
        ),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_SONNET_MODEL".to_owned(),
        Value::String(models.sonnet.clone()),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_OPUS_MODEL".to_owned(),
        Value::String(models.opus.clone()),
    );
    env.insert(
        "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_owned(),
        Value::String(models.haiku.clone()),
    );
    env.insert(
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_owned(),
        Value::String("1".to_owned()),
    );
    env.insert(
        "DISABLE_LOGIN_COMMAND".to_owned(),
        Value::String("1".to_owned()),
    );
    let model_overrides = object
        .entry("modelOverrides")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code settings modelOverrides must be a JSON object: {}",
                settings_path.display()
            )
        })?;
    model_overrides.insert(
        CLAUDE_OPUS_OVERRIDE_ID.to_owned(),
        Value::String(models.opus.clone()),
    );
    model_overrides.insert(
        CLAUDE_SONNET_OVERRIDE_ID.to_owned(),
        Value::String(models.sonnet.clone()),
    );
    model_overrides.insert(
        CLAUDE_HAIKU_OVERRIDE_ID.to_owned(),
        Value::String(models.haiku.clone()),
    );
    object.insert(
        "model".to_owned(),
        Value::String(CLAUDE_DEFAULT_MODEL.to_owned()),
    );
    object.insert(
        "codex_mixin_managed".to_owned(),
        json!({
            "marker": MANAGED_CLAUDE_MARKER,
            "env_keys": MANAGED_CLAUDE_ENV_KEYS,
            "model_override_keys": MANAGED_CLAUDE_MODEL_OVERRIDE_KEYS,
            "base_url": base_url,
            "model": CLAUDE_DEFAULT_MODEL,
            "models": {
                "opus": models.opus,
                "sonnet": models.sonnet,
                "haiku": models.haiku
            },
            "previous_env": previous_env,
            "previous_model": previous_model,
            "previous_model_overrides": previous_model_overrides
        }),
    );
    write_atomic_if_changed(&settings_path, &serde_json::to_vec_pretty(&settings)?)?;
    println!("claude code settings updated: {}", settings_path.display());
    println!("ANTHROPIC_BASE_URL: {base_url}");
    println!("claude code opus model: {}", models.opus);
    println!("claude code sonnet model: {}", models.sonnet);
    println!("claude code haiku model: {}", models.haiku);
    println!("claude code gateway auth: configured");
    println!("claude code nonessential traffic: disabled");
    println!("reload required: restart Claude Code or start a new session");
    Ok(())
}

pub(in crate::cli) fn sync_claude_hooks(settings_path: Option<PathBuf>) -> anyhow::Result<()> {
    let settings_path = resolve_claude_settings_path(settings_path)?;
    let enabled = reporting_enabled()?;
    if !enabled && !settings_path.exists() {
        return Ok(());
    }
    let mut settings: Value = if settings_path.exists() {
        serde_json::from_slice(&fs::read(&settings_path)?).map_err(|error| {
            anyhow::anyhow!(
                "invalid Claude Code settings {}: {error}",
                settings_path.display()
            )
        })?
    } else {
        json!({})
    };
    let object = settings.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "Claude Code settings must be a JSON object: {}",
            settings_path.display()
        )
    })?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Claude Code settings hooks must be an object"))?;
    for (event_name, event_argument) in [
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "user-prompt-submit"),
        ("PreToolUse", "pre-tool-use"),
        ("PostToolUse", "post-tool-use"),
        ("Stop", "stop"),
    ] {
        if let Some(groups) = hooks.get_mut(event_name).and_then(Value::as_array_mut) {
            for group in groups {
                if let Some(commands) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    commands.retain(|command| {
                        !command
                            .get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|value| value.contains(MANAGED_CLAUDE_HOOK_MARKER))
                    });
                }
            }
        }
        if enabled {
            let executable = std::env::current_exe()?;
            hooks
                .entry(event_name.to_owned())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("Claude Code hook event must be an array"))?
                .push(json!({
                    "hooks": [{
                        "type": "command",
                        "command": format!("'{}' report-hook --event {event_argument}", executable.to_string_lossy().replace('\'', "'\\''")),
                        "timeout": 30,
                        "statusMessage": "Reporting Baidu AI code usage"
                    }]
                }));
        }
    }
    write_atomic_if_changed(&settings_path, &serde_json::to_vec_pretty(&settings)?)?;
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
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code managed settings have no previous env backup: {}",
                settings_path.display()
            )
        })?;
    let previous_model = managed
        .as_ref()
        .and_then(|value| value.get("previous_model"))
        .filter(|value| !value.is_null())
        .cloned();
    let previous_model_overrides = match managed
        .as_ref()
        .and_then(|value| value.get("previous_model_overrides"))
    {
        None => Map::new(),
        Some(Value::Object(overrides)) => overrides.clone(),
        Some(_) => anyhow::bail!(
            "Claude Code managed previous model overrides must be an object: {}",
            settings_path.display()
        ),
    };
    let model_override_keys = managed_claude_keys(
        managed
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Claude Code managed settings are missing"))?,
        "model_override_keys",
        &[],
        &settings_path,
    )?;
    if let Some(env_value) = object.get_mut("env") {
        let env = env_value.as_object_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "Claude Code settings env must be a JSON object: {}",
                settings_path.display()
            )
        })?;
        for key in MANAGED_CLAUDE_ENV_KEYS {
            env.remove(*key);
        }
        env.extend(previous_env);
        if env.is_empty() {
            object.remove("env");
        }
    } else if !previous_env.is_empty() {
        object.insert("env".to_owned(), Value::Object(previous_env));
    }
    let remove_model_overrides = if let Some(overrides) = object.get_mut("modelOverrides") {
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
    if remove_model_overrides {
        object.remove("modelOverrides");
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
    println!("managed Claude Code settings restored; restart Claude Code to apply");
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
        println!("claude-code: installed via {}", settings_path.display());
    } else {
        println!("claude-code: not installed");
    }
    Ok(())
}

fn resolve_claude_models(
    config: &GatewayConfig,
    official_models: &[ProviderModel],
    request: ClaudeModelRequest,
) -> anyhow::Result<ClaudeModelMapping> {
    let registry = ProviderRegistry::new(config.providers.clone())?;
    let official_ids = official_models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    match request {
        ClaudeModelRequest::Automatic => select_default_claude_models(&registry, official_models),
        ClaudeModelRequest::Single(model) => {
            let model = validate_claude_model(&registry, &official_ids, &model)?;
            Ok(ClaudeModelMapping {
                opus: model.clone(),
                sonnet: model.clone(),
                haiku: model,
            })
        }
        ClaudeModelRequest::Mapping(models) => Ok(ClaudeModelMapping {
            opus: validate_claude_model(&registry, &official_ids, &models.opus)?,
            sonnet: validate_claude_model(&registry, &official_ids, &models.sonnet)?,
            haiku: validate_claude_model(&registry, &official_ids, &models.haiku)?,
        }),
    }
}

fn validate_claude_model(
    registry: &ProviderRegistry,
    official_ids: &std::collections::HashSet<&str>,
    model: &str,
) -> anyhow::Result<String> {
    if official_ids.contains(model) {
        return Ok(model.to_owned());
    }
    let resolved = registry
        .resolve_native_model(model)
        .ok_or_else(|| anyhow::anyhow!("requested Claude model is not configured: {model}"))?;
    Ok(resolved.catalog_slug.to_owned())
}

fn select_default_claude_models(
    registry: &ProviderRegistry,
    official_models: &[ProviderModel],
) -> anyhow::Result<ClaudeModelMapping> {
    let mut candidates = Vec::new();
    for resolved in registry.routable_models() {
        candidates.push((
            resolved.catalog_slug.to_owned(),
            resolved.upstream_model_id.to_ascii_lowercase(),
        ));
    }
    candidates.extend(
        official_models
            .iter()
            .map(|model| (model.id.clone(), model.id.to_ascii_lowercase())),
    );
    let fallback = candidates
        .iter()
        .find(|(_, model)| model.contains("claude"))
        .or_else(|| candidates.first())
        .map(|(slug, _)| slug.clone())
        .ok_or_else(|| anyhow::anyhow!("no enabled model is configured"))?;
    let select = |family: &str| {
        candidates
            .iter()
            .find(|(_, model)| model.contains(family))
            .map(|(slug, _)| slug.clone())
            .unwrap_or_else(|| fallback.clone())
    };
    Ok(ClaudeModelMapping {
        opus: select("opus"),
        sonnet: select("sonnet"),
        haiku: select("haiku"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_selected_official_models_without_provider_suffixes() {
        let mut provider = codex_mixin::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();
        provider.selected_models = vec!["custom-model".to_owned()];
        provider.cached_models = vec![ProviderModel {
            id: "custom-model".to_owned(),
            ..ProviderModel::default()
        }];
        let registry = ProviderRegistry::new(vec![provider]).unwrap();
        let official_ids = ["gpt-5.6-sol"].into_iter().collect();

        assert_eq!(
            validate_claude_model(&registry, &official_ids, "gpt-5.6-sol").unwrap(),
            "gpt-5.6-sol"
        );
    }
}
