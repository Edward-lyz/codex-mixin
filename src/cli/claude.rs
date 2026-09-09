use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::report_hook::reporting_enabled;
use crate::cli::runtime::effective_gateway_bind;
use codex_mixin::config::GatewayConfig;
use codex_mixin::gateway_access::GatewayClient;
use codex_mixin::provider::{ProviderDefinition, ProviderModel, catalog_model_slug};

use super::official_models::selected_official_models;

pub(in crate::cli) const MANAGED_CLAUDE_MARKER: &str = codex_mixin::clients::claude::MANAGED_MARKER;
const MANAGED_CLAUDE_HOOK_MARKER: &str = " report-hook --event ";
const CLAUDE_EXTENDED_CONTEXT_WINDOW: u64 = 1_000_000;

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

pub(in crate::cli) fn install_claude(settings_path: Option<PathBuf>) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::Claude;
    codex_mixin::application::client::install_with_client_key(client, || {
        let gateway_config = GatewayConfig::from_stored_config()?;
        let official_models = selected_official_models(&gateway_config)?;
        let gateway_bind = effective_gateway_bind(&gateway_config)?;
        install_claude_with_models(
            settings_path,
            &gateway_config,
            &official_models,
            gateway_bind,
            true,
        )
        .map(|_| ())
    })
}

#[cfg(test)]
pub(in crate::cli) fn install_claude_with_config(
    settings_path: Option<PathBuf>,
    gateway_config: &GatewayConfig,
) -> anyhow::Result<()> {
    install_claude_with_models(
        settings_path,
        gateway_config,
        &[],
        gateway_config.bind,
        true,
    )
    .map(|_| ())
}

fn install_claude_with_models(
    settings_path: Option<PathBuf>,
    gateway_config: &GatewayConfig,
    official_models: &[ProviderModel],
    gateway_bind: std::net::SocketAddr,
    announce: bool,
) -> anyhow::Result<bool> {
    let settings_path = resolve_claude_settings_path(settings_path)?;
    let (model_picker, default_model) = claude_model_picker(gateway_config, official_models)?;
    let base_url = format!("http://{gateway_bind}");
    let client_key = gateway_config.require_client_key(GatewayClient::Claude)?;
    let changed = codex_mixin::clients::claude::install(
        &settings_path,
        &base_url,
        &default_model,
        model_picker,
        &client_key,
    )?;
    if announce {
        println!("claude code settings updated: {}", settings_path.display());
        println!("ANTHROPIC_BASE_URL: {base_url}");
        println!("claude code default model: {default_model}");
        println!("claude code gateway auth: configured");
        println!("claude code nonessential traffic: disabled");
        println!("reload required: restart Claude Code or start a new session");
    }
    Ok(changed)
}

/// Re-render the managed Claude Code settings from the current provider
/// configuration. A missing or unmanaged settings file is left untouched.
pub(in crate::cli) fn sync_installed_claude_models() -> anyhow::Result<bool> {
    let gateway_config = GatewayConfig::from_stored_config()?;
    let official_models = selected_official_models(&gateway_config)?;
    let gateway_bind = effective_gateway_bind(&gateway_config)?;
    sync_claude_models(None, &gateway_config, &official_models, gateway_bind)
}

pub(in crate::cli) fn sync_claude_models(
    settings_path: Option<PathBuf>,
    gateway_config: &GatewayConfig,
    official_models: &[ProviderModel],
    gateway_bind: std::net::SocketAddr,
) -> anyhow::Result<bool> {
    let settings_path = resolve_claude_settings_path(settings_path)?;
    if !settings_path.exists() {
        return Ok(false);
    }
    if !fs::read_to_string(&settings_path)?.contains(MANAGED_CLAUDE_MARKER) {
        return Ok(false);
    }
    install_claude_with_models(
        Some(settings_path),
        gateway_config,
        official_models,
        gateway_bind,
        false,
    )
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
    codex_mixin::application::client::sync_managed_client_key(
        GatewayClient::Claude,
        || codex_mixin::clients::claude::is_managed(&settings_path),
        |key| codex_mixin::clients::claude::sync_client_key(&settings_path, key),
    )?;
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
    codex_mixin::clients::claude::uninstall(&settings_path)?;
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
