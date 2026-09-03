use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::report_hook::reporting_enabled;
use crate::cli::runtime::effective_gateway_bind;
use codex_mixin::config::GatewayConfig;
use codex_mixin::gateway_access::GatewayClient;
use codex_mixin::provider::{ProviderDefinition, ProviderModel, catalog_model_slug};

use super::official_models::selected_official_models;

pub(in crate::cli) const MANAGED_CLAUDE_MARKER: &str = "codex-mixin managed Claude Code";
const MANAGED_CLAUDE_HOOK_MARKER: &str = " report-hook --event ";
const CLAUDE_EXTENDED_CONTEXT_WINDOW: u64 = 1_000_000;
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
    "CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT",
    "DISABLE_LOGIN_COMMAND",
];
struct ClaudeSettingsBackup {
    previous_env: Map<String, Value>,
    previous_model: Option<Value>,
    managed_env_keys: Vec<String>,
    previous_model_overrides: Map<String, Value>,
    managed_model_override_keys: Vec<String>,
    previous_model_picker: Option<Value>,
    manages_model_picker: bool,
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

pub(in crate::cli) fn install_claude(settings_path: Option<PathBuf>) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::Claude;
    let key_existed = codex_mixin::config::gateway_client_key_exists(client)?;
    codex_mixin::config::ensure_gateway_client_key(client)?;
    let result = (|| {
        let gateway_config = GatewayConfig::from_stored_config()?;
        let official_models = selected_official_models(&gateway_config)?;
        let gateway_bind = effective_gateway_bind(&gateway_config)?;
        install_claude_with_models(
            settings_path,
            &gateway_config,
            &official_models,
            gateway_bind,
        )
    })();
    super::rollback_new_client_key_on_error(result, client, key_existed)
}

#[cfg(test)]
pub(in crate::cli) fn install_claude_with_config(
    settings_path: Option<PathBuf>,
    gateway_config: &GatewayConfig,
) -> anyhow::Result<()> {
    install_claude_with_models(settings_path, gateway_config, &[], gateway_config.bind)
}

fn install_claude_with_models(
    settings_path: Option<PathBuf>,
    gateway_config: &GatewayConfig,
    official_models: &[ProviderModel],
    gateway_bind: std::net::SocketAddr,
) -> anyhow::Result<()> {
    let settings_path = resolve_claude_settings_path(settings_path)?;
    let (model_picker, default_model) = claude_model_picker(gateway_config, official_models)?;
    let managed_model_override_keys = Vec::<String>::new();
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
                previous_model_picker: managed
                    .get("previous_model_picker")
                    .filter(|value| !value.is_null())
                    .cloned(),
                manages_model_picker: managed.get("model_picker_managed").and_then(Value::as_bool)
                    == Some(true),
            })
        })
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
            .chain(MANAGED_CLAUDE_ENV_KEYS.iter().map(|key| (*key).to_owned()))
            .collect::<BTreeSet<_>>();
        for key in env_keys {
            let was_managed = existing_backup.as_ref().is_some_and(|backup| {
                backup
                    .managed_env_keys
                    .iter()
                    .any(|managed_key| managed_key == &key)
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
        let keys = existing_backup
            .as_ref()
            .map(|backup| backup.managed_model_override_keys.iter().cloned().collect())
            .unwrap_or_else(BTreeSet::new)
            .into_iter()
            .chain(managed_model_override_keys.iter().cloned())
            .collect::<BTreeSet<_>>();
        for key in keys {
            let was_managed = existing_backup.as_ref().is_some_and(|backup| {
                backup
                    .managed_model_override_keys
                    .iter()
                    .any(|managed_key| managed_key == &key)
            });
            if let Some(value) = overrides.remove(&key)
                && !was_managed
            {
                previous_model_overrides.insert(key, value);
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
        Value::String(gateway_config.require_client_key(GatewayClient::Claude)?),
    );
    env.insert(
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_owned(),
        Value::String("1".to_owned()),
    );
    env.insert(
        "CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT".to_owned(),
        Value::String("1".to_owned()),
    );
    env.insert(
        "DISABLE_LOGIN_COMMAND".to_owned(),
        Value::String("1".to_owned()),
    );
    object.insert("model".to_owned(), Value::String(default_model.clone()));
    object.insert("modelPicker".to_owned(), model_picker);
    object.insert(
        "codex_mixin_managed".to_owned(),
        json!({
            "marker": MANAGED_CLAUDE_MARKER,
            "env_keys": MANAGED_CLAUDE_ENV_KEYS,
            "model_override_keys": managed_model_override_keys,
            "base_url": base_url,
            "model": default_model,
            "model_picker_managed": true,
            "previous_env": previous_env,
            "previous_model": previous_model,
            "previous_model_picker": previous_model_picker,
            "previous_model_overrides": previous_model_overrides
        }),
    );
    write_atomic_if_changed(&settings_path, &serde_json::to_vec_pretty(&settings)?)?;
    println!("claude code settings updated: {}", settings_path.display());
    println!("ANTHROPIC_BASE_URL: {base_url}");
    println!("claude code default model: {default_model}");
    println!("claude code gateway auth: configured");
    println!("claude code nonessential traffic: disabled");
    println!("reload required: restart Claude Code or start a new session");
    Ok(())
}

fn claude_model_picker(
    config: &GatewayConfig,
    official_models: &[ProviderModel],
) -> anyhow::Result<(Value, String)> {
    let mut options = Vec::new();
    let mut picker_models = BTreeSet::new();
    for provider in config.providers.iter().filter(|provider| provider.enabled) {
        for model in provider.cached_models.iter().filter(|model| {
            provider
                .selected_models
                .iter()
                .any(|selected| selected == &model.id)
        }) {
            let target = catalog_model_slug(&model.id, &provider.id);
            if !picker_models.insert(target.clone()) {
                continue;
            }
            options.push(json!({
                "model": claude_picker_model(&target, model.context_window),
                "label": claude_picker_label(provider, model),
                "description": claude_picker_description(provider, model),
            }));
        }
    }
    for model in official_models {
        if !picker_models.insert(model.id.clone()) {
            continue;
        }
        options.push(json!({
            "model": claude_picker_model(&model.id, model.context_window),
            "label": model.display_name.as_deref().unwrap_or(&model.id),
            "description": claude_picker_description_text("OpenAI official", model.context_window),
        }));
    }
    options.sort_by(|left, right| {
        left.get("label")
            .and_then(Value::as_str)
            .cmp(&right.get("label").and_then(Value::as_str))
    });
    let default_model = options
        .first()
        .and_then(|option| option.get("model"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("no enabled and selected Claude Code model is configured"))?
        .to_owned();
    Ok((
        json!({
            "replaceBuiltInOptions": true,
            "options": options,
        }),
        default_model,
    ))
}

fn claude_picker_model(model: &str, context_window: Option<u64>) -> String {
    if context_window.is_some_and(|window| window >= CLAUDE_EXTENDED_CONTEXT_WINDOW) {
        return format!("{model}[1m]");
    }
    model.to_owned()
}

fn claude_picker_label<'a>(provider: &ProviderDefinition, model: &'a ProviderModel) -> &'a str {
    if provider.preset_id.as_deref() == Some("baidu-oneapi") {
        return &model.id;
    }
    model.display_name.as_deref().unwrap_or(&model.id)
}

fn claude_picker_description(provider: &ProviderDefinition, model: &ProviderModel) -> String {
    let description = if provider.preset_id.as_deref() != Some("aws-bedrock") {
        provider.display_name.clone()
    } else if model.id.contains(":application-inference-profile/") {
        let profile_id = model.id.rsplit('/').next().unwrap_or(&model.id);
        let scope = if model
            .aliases
            .iter()
            .any(|alias| alias.starts_with("global."))
        {
            "Global"
        } else if model.aliases.iter().any(|alias| alias.starts_with("us.")) {
            "US"
        } else {
            ""
        };
        if scope.is_empty() {
            format!("Discount \u{b7} {profile_id}")
        } else {
            format!("Discount {scope} \u{b7} {profile_id}")
        }
    } else {
        let inference_profile_id = model
            .id
            .split_once(":inference-profile/")
            .map(|(_, profile_id)| profile_id)
            .unwrap_or(&model.id);
        if inference_profile_id.starts_with("global.") {
            "AWS Global".to_owned()
        } else if inference_profile_id.starts_with("us.") {
            "AWS US".to_owned()
        } else if model.id.contains(":inference-profile/") {
            "AWS Inference Profile".to_owned()
        } else {
            "AWS Foundation".to_owned()
        }
    };
    claude_picker_description_text(&description, model.context_window)
}

fn claude_picker_description_text(description: &str, context_window: Option<u64>) -> String {
    let Some(context_window) = context_window else {
        return description.to_owned();
    };
    let context = if context_window % 1_000_000 == 0 {
        format!("{}M", context_window / 1_000_000)
    } else if context_window % 1_000 == 0 {
        format!("{}K", context_window / 1_000)
    } else {
        context_window.to_string()
    };
    format!("{description} \u{b7} {context} context")
}

pub(in crate::cli) fn sync_installed_claude_client_key() -> anyhow::Result<()> {
    let settings_path = resolve_claude_settings_path(None)?;
    if !settings_path.exists() {
        return Ok(());
    }
    let raw = fs::read(&settings_path)?;
    if !String::from_utf8_lossy(&raw).contains(MANAGED_CLAUDE_MARKER) {
        return Ok(());
    }
    let mut settings: Value = serde_json::from_slice(&raw)?;
    let object = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Claude Code settings must be a JSON object"))?;
    if object
        .get("codex_mixin_managed")
        .and_then(|managed| managed.get("marker"))
        .and_then(Value::as_str)
        != Some(MANAGED_CLAUDE_MARKER)
    {
        return Ok(());
    }
    let client_key = codex_mixin::config::ensure_gateway_client_key(
        codex_mixin::gateway_access::GatewayClient::Claude,
    )?;
    object
        .get_mut("env")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("managed Claude Code env is missing"))?
        .insert("ANTHROPIC_AUTH_TOKEN".to_owned(), Value::String(client_key));
    write_atomic_if_changed(&settings_path, &serde_json::to_vec_pretty(&settings)?)?;
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
    let manages_model_picker = managed
        .as_ref()
        .and_then(|value| value.get("model_picker_managed"))
        .and_then(Value::as_bool)
        == Some(true);
    let previous_model_picker = managed
        .as_ref()
        .and_then(|value| value.get("previous_model_picker"))
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
    let env_keys = managed_claude_keys(
        managed
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Claude Code managed settings are missing"))?,
        "env_keys",
        LEGACY_MANAGED_CLAUDE_ENV_KEYS,
        &settings_path,
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_uses_baidu_model_id_instead_of_marketing_description() {
        let provider = codex_mixin::provider::baidu_oneapi_provider("baidu", "key");
        let model = ProviderModel {
            id: "GLM-5.3".to_owned(),
            display_name: Some("GLM latest flagship model".to_owned()),
            ..ProviderModel::default()
        };

        assert_eq!(claude_picker_label(&provider, &model), "GLM-5.3");
    }

    #[test]
    fn picker_describes_application_profile_scope() {
        let provider = codex_mixin::provider::aws_bedrock_aksk_provider(
            "aws-bedrock",
            "access-key",
            "secret-key",
            None,
            "us-east-2",
        );
        let model = ProviderModel {
            id: "arn:aws:bedrock:us-east-2:123:application-inference-profile/abc".to_owned(),
            aliases: vec!["us.anthropic.claude-opus-5-20251101-v1:0".to_owned()],
            ..ProviderModel::default()
        };
        assert_eq!(
            claude_picker_description(&provider, &model),
            "Discount US \u{b7} abc"
        );
    }
}
