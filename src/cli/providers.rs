use std::collections::HashSet;

use codex_mixin::config::{StoredGatewayConfig, load_stored_config, mutate_stored_config};
use codex_mixin::provider::{
    ProviderModel, ProviderModelSource, ProviderPreset, ProviderProtocol, ProviderQuotaParser,
    apply_discovered_models, discover_provider_models, redact_provider_error,
};
use codex_mixin::web_search::WebSearchCapabilities;
use serde_json::json;

use super::config_input::{normalize_base_url, trim_required};

#[derive(Clone, Debug)]
pub(super) struct AddProviderOptions {
    pub(super) preset: String,
    pub(super) id: Option<String>,
    pub(super) key: String,
    pub(super) display_name: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) protocol: Option<String>,
    pub(super) api_path: Option<String>,
    pub(super) models_path: Option<String>,
    pub(super) image_generation_path: Option<String>,
    pub(super) quota_url: Option<String>,
    pub(super) quota_username: Option<String>,
    pub(super) quota_currency: Option<String>,
    pub(super) quota_parser: Option<String>,
    pub(super) gateway_key: Option<String>,
    pub(super) static_models: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct UpdateProviderOptions {
    pub(super) id: String,
    pub(super) key: Option<String>,
    pub(super) clear_key: bool,
    pub(super) display_name: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) protocol: Option<String>,
    pub(super) api_path: Option<String>,
    pub(super) models_path: Option<String>,
    pub(super) image_generation_path: Option<String>,
    pub(super) clear_image_generation: bool,
    pub(super) quota_url: Option<String>,
    pub(super) clear_quota: bool,
    pub(super) quota_username: Option<String>,
    pub(super) quota_currency: Option<String>,
    pub(super) quota_parser: Option<String>,
}

pub(super) fn list_providers(json_output: bool) -> anyhow::Result<()> {
    let config = load_stored_config()?.unwrap_or_default();
    if json_output {
        let providers = config
            .providers
            .iter()
            .map(|provider| {
                let readiness = provider.readiness();
                let available_models = provider
                    .cached_models
                    .iter()
                    .map(|model| model.id.as_str())
                    .collect::<HashSet<_>>();
                let unavailable_selected_models = provider
                    .selected_models
                    .iter()
                    .filter(|model| !available_models.contains(model.as_str()))
                    .collect::<Vec<_>>();
                json!({
                    "id": provider.id,
                    "display_name": provider.display_name,
                    "enabled": provider.enabled,
                    "preset_id": provider.preset_id,
                    "protocol": provider.protocol,
                    "base_url": provider.base_url,
                    "api_path": provider.api_path,
                    "model_source": provider.model_source,
                    "api_key_configured": !provider.auth.api_key.is_empty(),
                    "image_generation_path": provider.image_generation_path,
                    "quota_url": provider.quota_url,
                    "quota_username": provider.quota_username,
                    "quota_currency": provider.quota_currency,
                    "quota_parser": provider.quota_parser,
                    "selected_models": provider.selected_models,
                    "new_models": provider.new_models,
                    "unavailable_selected_models": unavailable_selected_models,
                    "cached_models": provider.cached_models,
                    "models_refreshed_at_ms": provider.models_refreshed_at_ms,
                    "last_model_refresh_error": provider.models_refresh_error,
                    "readiness": readiness.status,
                    "readiness_issues": readiness.issues,
                    "routable_model_count": readiness.routable_model_count,
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "config_version": config.config_version,
                "gateway_bind": config.gateway_bind,
                "gateway_auth_configured": config.gateway_api_key.is_some(),
                "providers": providers,
            }))?
        );
        return Ok(());
    }
    if config.providers.is_empty() {
        println!("no providers configured");
        return Ok(());
    }
    for provider in config.providers {
        let readiness = provider.readiness();
        println!(
            "{}\t{}\t{}\t{}\t{}/{} selected\t{} routable\t{}",
            provider.id,
            provider.display_name,
            if provider.enabled {
                "enabled"
            } else {
                "disabled"
            },
            protocol_name(provider.protocol),
            provider.selected_models.len(),
            provider.cached_models.len(),
            readiness.routable_model_count,
            readiness.status.as_str(),
        );
    }
    Ok(())
}

pub(super) fn add_provider(options: AddProviderOptions) -> anyhow::Result<()> {
    let preset = ProviderPreset::parse(options.preset.trim())?;
    let id = options.id.unwrap_or_else(|| preset.default_id().to_owned());
    let mut provider = preset.create(id.clone(), trim_required("key", options.key)?);
    if let Some(display_name) = options.display_name {
        provider.display_name = trim_required("display name", display_name)?;
    }
    if let Some(base_url) = options.base_url {
        provider.base_url = normalize_base_url(base_url)?;
    }
    if preset == ProviderPreset::Custom && provider.base_url.is_empty() {
        anyhow::bail!("custom provider requires --base-url");
    }
    if let Some(protocol) = options.protocol {
        provider.protocol = parse_protocol(&protocol)?;
    }
    if let Some(api_path) = options.api_path {
        provider.api_path = normalize_path("API path", api_path)?;
    }
    if let Some(models_path) = options.models_path {
        provider.model_source = ProviderModelSource::OpenAiCompatible {
            path: normalize_path("models path", models_path)?,
        };
    }
    if !options.static_models.is_empty() {
        let models = normalize_model_ids(options.static_models)?;
        provider.model_source = ProviderModelSource::Static;
        provider.cached_models = models
            .iter()
            .map(|id| ProviderModel {
                id: id.clone(),
                ..ProviderModel::default()
            })
            .collect();
        provider.selected_models = models;
    }
    if let Some(path) = options.image_generation_path {
        provider.image_generation_path = Some(normalize_path("image generation path", path)?);
    }
    if let Some(quota_url) = options.quota_url {
        provider.quota_url = Some(normalize_base_url(quota_url)?);
    }
    if let Some(username) = options.quota_username {
        provider.quota_username = Some(trim_required("quota username", username)?);
    }
    if let Some(currency) = options.quota_currency {
        provider.quota_currency = Some(normalize_currency(currency)?);
    }
    if let Some(parser) = options.quota_parser {
        provider.quota_parser = parse_quota_parser(&parser)?;
    }
    provider.validate()?;
    let gateway_api_key = options
        .gateway_key
        .map(|key| trim_required("gateway key", key))
        .transpose()?;
    mutate_and_invalidate(|config| {
        if config.providers.iter().any(|provider| provider.id == id) {
            anyhow::bail!("provider already exists: {id}");
        }
        if gateway_api_key.is_some() {
            config.gateway_api_key = gateway_api_key;
        }
        config.providers.push(provider);
        Ok(())
    })?;
    println!("provider added: {id}");
    Ok(())
}

pub(super) fn update_provider(options: UpdateProviderOptions) -> anyhow::Result<()> {
    let id = options.id.clone();
    mutate_and_invalidate(|config| {
        let provider = find_provider_mut(config, &id)?;
        if options.clear_key {
            provider.auth.api_key.clear();
        } else if let Some(key) = options.key {
            provider.auth.api_key = trim_required("key", key)?;
        }
        if let Some(display_name) = options.display_name {
            provider.display_name = trim_required("display name", display_name)?;
        }
        if let Some(base_url) = options.base_url {
            provider.base_url = normalize_base_url(base_url)?;
        }
        if let Some(protocol) = options.protocol {
            provider.protocol = parse_protocol(&protocol)?;
        }
        if let Some(api_path) = options.api_path {
            provider.api_path = normalize_path("API path", api_path)?;
        }
        if let Some(models_path) = options.models_path {
            provider.model_source = ProviderModelSource::OpenAiCompatible {
                path: normalize_path("models path", models_path)?,
            };
        }
        if options.clear_image_generation {
            provider.image_generation_path = None;
        } else if let Some(path) = options.image_generation_path {
            provider.image_generation_path = Some(normalize_path("image generation path", path)?);
        }
        if options.clear_quota {
            provider.quota_url = None;
            provider.quota_username = None;
            provider.quota_currency = None;
            provider.quota_parser = ProviderQuotaParser::Generic;
        } else {
            if let Some(quota_url) = options.quota_url {
                provider.quota_url = Some(normalize_base_url(quota_url)?);
            }
            if let Some(username) = options.quota_username {
                provider.quota_username = Some(trim_required("quota username", username)?);
            }
            if let Some(currency) = options.quota_currency {
                provider.quota_currency = Some(normalize_currency(currency)?);
            }
            if let Some(parser) = options.quota_parser {
                provider.quota_parser = parse_quota_parser(&parser)?;
            }
        }
        provider.validate()
    })?;
    println!("provider updated: {id}");
    Ok(())
}

pub(super) fn set_provider_enabled(id: &str, enabled: bool) -> anyhow::Result<()> {
    mutate_and_invalidate(|config| {
        ensure_has_providers(config)?;
        find_provider_mut(config, id)?.enabled = enabled;
        Ok(())
    })?;
    println!(
        "provider {}: {id}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

pub(super) fn remove_provider(id: &str) -> anyhow::Result<()> {
    mutate_and_invalidate(|config| {
        ensure_has_providers(config)?;
        let previous_len = config.providers.len();
        config.providers.retain(|provider| provider.id != id);
        if config.providers.len() == previous_len {
            anyhow::bail!("unknown provider: {id}");
        }
        Ok(())
    })?;
    println!("provider removed: {id}");
    Ok(())
}

pub(super) async fn discover_models(id: &str) -> anyhow::Result<()> {
    let config = required_config()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?
        .clone();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let models = match discover_provider_models(&client, &provider).await {
        Ok(models) => models,
        Err(error) => {
            let stored_error = redact_provider_error(&provider, &format!("{error:#}"));
            mutate_and_invalidate(|config| {
                let current = find_provider_mut(config, id)?;
                anyhow::ensure!(
                    discovery_settings_match(current, &provider),
                    "provider {id} discovery settings changed during refresh; retry"
                );
                current.models_refresh_error = Some(stored_error);
                Ok(())
            })?;
            return Err(error);
        }
    };
    let count = models.len();
    mutate_and_invalidate(|config| {
        let current = find_provider_mut(config, id)?;
        anyhow::ensure!(
            discovery_settings_match(current, &provider),
            "provider {id} discovery settings changed during refresh; retry"
        );
        apply_discovered_models(current, models)
    })?;
    println!("provider models refreshed: {id} ({count} available)");
    Ok(())
}

pub(super) async fn test_provider(id: &str, json_output: bool) -> anyhow::Result<()> {
    let config = required_config()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
    provider.validate()?;
    let (mode, model_count) = match &provider.model_source {
        ProviderModelSource::Static => ("configuration", provider.cached_models.len()),
        _ => {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?;
            let models = discover_provider_models(&client, provider).await?;
            ("models_endpoint", models.len())
        }
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider_id": provider.id,
                "ok": true,
                "mode": mode,
                "model_count": model_count,
                "paid_inference_performed": false,
            }))?
        );
    } else if mode == "configuration" {
        println!(
            "provider test ok: {id} (static model source; configuration only, no paid inference)"
        );
    } else {
        println!("provider test ok: {id} ({model_count} models)");
    }
    Ok(())
}

pub(super) fn select_models(id: &str, models: Vec<String>) -> anyhow::Result<()> {
    let models = normalize_model_ids(models)?;
    let selected_count = models.len();
    mutate_and_invalidate(|config| {
        ensure_has_providers(config)?;
        apply_model_selection(find_provider_mut(config, id)?, models)
    })?;
    println!("provider models selected: {id} ({selected_count})");
    Ok(())
}

fn apply_model_selection(
    provider: &mut codex_mixin::provider::ProviderDefinition,
    models: Vec<String>,
) -> anyhow::Result<()> {
    let allowed = provider
        .cached_models
        .iter()
        .map(|model| model.id.as_str())
        .chain(provider.selected_models.iter().map(String::as_str))
        .collect::<HashSet<_>>();
    for model in &models {
        if !allowed.contains(model.as_str()) {
            anyhow::bail!(
                "provider {} has no known model {model}; run discover first",
                provider.id
            );
        }
    }
    provider.selected_models = models;
    provider.new_models.clear();
    provider.validate()
}

fn required_config() -> anyhow::Result<StoredGatewayConfig> {
    load_stored_config()?.ok_or_else(|| anyhow::anyhow!("provider configuration is missing"))
}

fn ensure_has_providers(config: &StoredGatewayConfig) -> anyhow::Result<()> {
    if config.providers.is_empty() {
        anyhow::bail!("provider configuration is missing");
    }
    Ok(())
}

fn find_provider_mut<'a>(
    config: &'a mut StoredGatewayConfig,
    id: &str,
) -> anyhow::Result<&'a mut codex_mixin::provider::ProviderDefinition> {
    config
        .providers
        .iter_mut()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))
}

fn mutate_and_invalidate<T>(
    mutation: impl FnOnce(&mut StoredGatewayConfig) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let result = mutate_stored_config(mutation)?;
    WebSearchCapabilities::clear_default_cache()?;
    Ok(result)
}

fn discovery_settings_match(
    current: &codex_mixin::provider::ProviderDefinition,
    discovered_from: &codex_mixin::provider::ProviderDefinition,
) -> bool {
    current.base_url == discovered_from.base_url
        && current.model_source == discovered_from.model_source
        && current.auth == discovered_from.auth
}

fn parse_protocol(value: &str) -> anyhow::Result<ProviderProtocol> {
    match value.trim() {
        "anthropic_messages" | "anthropic" => Ok(ProviderProtocol::AnthropicMessages),
        "open_ai_chat" | "openai_chat" | "chat" => Ok(ProviderProtocol::OpenAiChat),
        "open_ai_responses" | "openai_responses" | "responses" => {
            Ok(ProviderProtocol::OpenAiResponses)
        }
        other => anyhow::bail!("unsupported provider protocol: {other}"),
    }
}

fn protocol_name(protocol: ProviderProtocol) -> &'static str {
    match protocol {
        ProviderProtocol::AnthropicMessages => "anthropic_messages",
        ProviderProtocol::OpenAiChat => "open_ai_chat",
        ProviderProtocol::OpenAiResponses => "open_ai_responses",
    }
}

fn parse_quota_parser(value: &str) -> anyhow::Result<ProviderQuotaParser> {
    match value.trim() {
        "generic" => Ok(ProviderQuotaParser::Generic),
        "baidu_oneapi" | "baidu-oneapi" => Ok(ProviderQuotaParser::BaiduOneApi),
        "openrouter" => Ok(ProviderQuotaParser::OpenRouter),
        other => anyhow::bail!("unsupported quota parser: {other}"),
    }
}

fn normalize_currency(value: String) -> anyhow::Result<String> {
    let currency = trim_required("quota currency", value)?.to_ascii_uppercase();
    anyhow::ensure!(
        currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_uppercase()),
        "quota currency must be a three-letter code"
    );
    Ok(currency)
}

fn normalize_path(label: &str, value: String) -> anyhow::Result<String> {
    let value = trim_required(label, value)?;
    Ok(if value.starts_with('/') {
        value
    } else {
        format!("/{value}")
    })
}

fn normalize_model_ids(models: Vec<String>) -> anyhow::Result<Vec<String>> {
    let mut normalized = Vec::with_capacity(models.len());
    let mut seen = HashSet::with_capacity(models.len());
    for model in models {
        let model = trim_required("model", model)?;
        if seen.insert(model.clone()) {
            normalized.push(model);
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_selection_can_preserve_or_remove_an_unavailable_selected_model() {
        let mut provider = codex_mixin::provider::open_code_go_provider("provider", "key");
        provider.selected_models.push("temporarily-gone".to_owned());
        provider.new_models = vec!["new-model".to_owned()];
        provider.cached_models.push(ProviderModel {
            id: "new-model".to_owned(),
            ..ProviderModel::default()
        });

        apply_model_selection(
            &mut provider,
            vec!["glm-5.2".to_owned(), "temporarily-gone".to_owned()],
        )
        .unwrap();
        assert_eq!(provider.selected_models, ["glm-5.2", "temporarily-gone"]);
        assert!(provider.new_models.is_empty());

        apply_model_selection(&mut provider, vec!["glm-5.2".to_owned()]).unwrap();
        assert_eq!(provider.selected_models, ["glm-5.2"]);
    }

    #[test]
    fn discovery_errors_are_bounded_and_redact_the_provider_key() {
        let provider = codex_mixin::provider::open_code_go_provider("provider", "secret-key");
        let error = format!("request used secret-key: {}", "x".repeat(20_000));

        let redacted = redact_provider_error(&provider, &error);

        assert!(!redacted.contains("secret-key"));
        assert!(redacted.contains("<redacted>"));
        assert_eq!(redacted.chars().count(), 8_000);
    }
}
