use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};

use super::super::ConfigScope;
use super::super::runtime::*;

pub(crate) fn show_config(json_output: bool, scope: ConfigScope) -> anyhow::Result<()> {
    let path = stored_config_path();
    match scope {
        ConfigScope::Stored => {
            let stored = load_stored_config()?.unwrap_or_default();
            let providers = redacted_providers(&stored.providers);
            let value = serde_json::json!({
                "path": path,
                "config_version": stored.config_version,
                "gateway_bind": stored.gateway_bind,
                "gateway_api_key": stored.gateway_api_key.as_ref().map(|_| "<redacted>"),
                "providers": providers,
                "fusion_profiles": stored.fusion_profiles
            });
            print_config_value(json_output, &value)
        }
        ConfigScope::Effective => {
            let config = GatewayConfig::from_stored_config()?;
            let bind = match load_runtime_metadata()? {
                Some(runtime) if pid_is_running(runtime.pid)? => runtime.bind,
                _ => config.bind,
            };
            let providers = redacted_providers(&config.providers);
            let value = serde_json::json!({
                "path": path,
                "bind": bind.to_string(),
                "providers": providers,
                "official_image_generation_url": config.official_image_generation_url()?,
                "official_image_edit_url": config.official_image_edit_url()?,
                "official_responses_url": config.official_responses_url,
                "codex_auth_path": config.codex_auth_path,
                "gateway_api_key": config.gateway_api_key.as_ref().map(|_| "<redacted>"),
                "accept_codex_oauth": config.accept_codex_oauth,
                "thinking_mode": format!("{:?}", config.thinking_mode),
                "enable_web_search_tool": config.enable_web_search_tool,
                "web_search_tool_type": config.web_search_tool_type,
                "web_search_max_uses": config.web_search_max_uses
            });
            print_config_value(json_output, &value)
        }
    }
}

pub(crate) fn redacted_providers(
    providers: &[codex_mixin::provider::ProviderDefinition],
) -> Vec<serde_json::Value> {
    providers
        .iter()
        .map(|provider| {
            serde_json::json!({
                "id": provider.id,
                "display_name": provider.display_name,
                "enabled": provider.enabled,
                "preset_id": provider.preset_id,
                "protocol": provider.protocol,
                "base_url": provider.base_url,
                "api_path": provider.api_path,
                "model_source": provider.model_source,
                "api_key": if provider.auth.api_key.is_empty() { "<missing>" } else { "<redacted>" },
                "aws_sigv4": provider.auth.aws_sigv4.as_ref().map(|aws| serde_json::json!({
                    "access_key_id": "<redacted>",
                    "secret_access_key": "<redacted>",
                    "session_token": aws.session_token.as_ref().map(|_| "<redacted>"),
                    "region": aws.region,
                    "service": aws.service,
                })),
                "image_generation_path": provider.image_generation_path,
                "quota_url": provider.quota_url,
                "quota_username": provider.quota_username,
                "quota_workspace_id": provider.quota_workspace_id,
                "quota_auth_cookie": provider
                    .quota_auth_cookie
                    .as_ref()
                    .map(|_| "<redacted>"),
                "quota_currency": provider.quota_currency,
                "selected_models": provider.selected_models,
                "new_models": provider.new_models,
                "cached_models": provider.cached_models,
                "models_refreshed_at_ms": provider.models_refreshed_at_ms,
                "last_model_refresh_error": provider.models_refresh_error,
                "readiness": provider.readiness(),
            })
        })
        .collect()
}

fn print_config_value(json_output: bool, value: &serde_json::Value) -> anyhow::Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(value)?);
        return Ok(());
    }
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("config output must be an object"))?;
    for (key, value) in object {
        println!("{key}: {}", printable_json_value(value));
    }
    Ok(())
}

fn printable_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => "<unset>".to_owned(),
        other => other.to_string(),
    }
}
