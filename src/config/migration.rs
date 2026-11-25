use super::StoredGatewayConfig;
use crate::config::{
    ensure_config_version, validate_failover_model_references, validate_fusion_model_references,
};
use crate::provider::CONFIG_VERSION;
use crate::provider::ProviderModel;
use crate::provider::ProviderModelSource;
use crate::provider::ProviderPreset;
use crate::provider::ProviderProtocol;
use crate::provider::ProviderQuotaParser;
use crate::provider::ProviderRegistry;
use crate::provider::is_managed_pi_provider;
use anyhow::{Context, anyhow};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
struct LegacyStoredGatewayConfig {
    #[serde(default)]
    gateway_bind: Option<String>,
    #[serde(default)]
    provider_preset: Option<String>,
    #[serde(default)]
    upstream_kind: Option<String>,
    #[serde(default)]
    upstream_base_url: Option<String>,
    #[serde(default)]
    upstream_messages_path: Option<String>,
    #[serde(default)]
    upstream_models_path: Option<String>,
    #[serde(default)]
    upstream_image_generation_path: Option<String>,
    #[serde(default)]
    upstream_api_key: Option<String>,
    #[serde(default)]
    gateway_api_key: Option<String>,
    #[serde(default)]
    quota_url: Option<String>,
    #[serde(default)]
    quota_username: Option<String>,
    #[serde(default)]
    fusion_profiles: Vec<crate::config::FusionProfile>,
}
pub(super) fn parse_stored_config(raw: &str) -> anyhow::Result<StoredGatewayConfig> {
    let mut document: serde_json::Value = serde_json::from_str(raw)?;
    migrate_legacy_ducc_config(&mut document);
    if let Some(version) = document
        .get("config_version")
        .and_then(serde_json::Value::as_u64)
    {
        let mut parsed: StoredGatewayConfig = serde_json::from_value(document)?;
        ensure_config_version(u32::try_from(version).context("config_version is too large")?)?;
        upgrade_deepseek_quota_defaults(&mut parsed);
        upgrade_deepseek_responses_endpoint(&mut parsed);
        upgrade_agy_endpoint(&mut parsed);
        upgrade_opencode_go_quota_defaults(&mut parsed);
        upgrade_opencode_go_transport(&mut parsed);
        upgrade_baidu_image_generation_defaults(&mut parsed);
        upgrade_aws_bedrock_model_source(&mut parsed);
        upgrade_aws_bedrock_sigv4_endpoint(&mut parsed);
        bootstrap_unrefreshed_selected_models(&mut parsed);
        backfill_data_report_executable(&mut parsed);
        drop_invalid_pi_plugin_providers(&mut parsed);
        parsed.apply_model_order();
        super::validate_cors_origins(&parsed.cors_allowed_origins)?;
        return Ok(parsed);
    } else if document.get("config_version").is_some() {
        anyhow::bail!("config_version must be an unsigned integer");
    }
    let object = document
        .as_object()
        .ok_or_else(|| anyhow!("stored configuration must be a JSON object"))?;
    let is_legacy = [
        "provider_preset",
        "upstream_kind",
        "upstream_base_url",
        "upstream_messages_path",
        "upstream_models_path",
        "upstream_api_key",
        "quota_url",
        "quota_username",
    ]
    .iter()
    .any(|field| object.contains_key(*field));
    if !is_legacy {
        anyhow::bail!(
            "configuration has no config_version and does not match the legacy single-provider format"
        );
    }
    let mut migrated = migrate_legacy_config(serde_json::from_value(document)?)?;
    upgrade_deepseek_quota_defaults(&mut migrated);
    upgrade_deepseek_responses_endpoint(&mut migrated);
    upgrade_agy_endpoint(&mut migrated);
    upgrade_opencode_go_quota_defaults(&mut migrated);
    upgrade_opencode_go_transport(&mut migrated);
    bootstrap_unrefreshed_selected_models(&mut migrated);
    backfill_data_report_executable(&mut migrated);
    migrated.apply_model_order();
    Ok(migrated)
}

fn upgrade_baidu_image_generation_defaults(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() == Some("baidu-oneapi")
            && provider.image_generation_path.is_none()
        {
            provider.image_generation_path = Some("/v1/images/generations".to_owned());
        }
    }
}
fn upgrade_aws_bedrock_model_source(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() == Some("aws-bedrock")
            && provider.auth.aws_sigv4.is_some()
            && provider.model_source == ProviderModelSource::Static
        {
            provider.model_source = ProviderModelSource::AwsBedrock;
        }
    }
}

/// Move SigV4 (AK/SK) Bedrock providers off the Mantle host.
///
/// The preset used to point AK/SK providers at the Mantle Anthropic endpoint,
/// which only serves Bedrock API keys and answers 404 for every Claude model
/// when signed with SigV4. Rewrite only the untouched preset default; a
/// customized base URL stays as the user's own choice.
fn upgrade_aws_bedrock_sigv4_endpoint(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() != Some("aws-bedrock") {
            continue;
        }
        let Some(aws) = provider.auth.aws_sigv4.as_mut() else {
            continue;
        };
        if aws.service == crate::provider::AWS_BEDROCK_MANTLE_SERVICE
            && provider.base_url == crate::provider::aws_bedrock_mantle_base_url(&aws.region)
        {
            provider.base_url = crate::provider::aws_bedrock_runtime_base_url(&aws.region);
            aws.service = crate::provider::AWS_BEDROCK_RUNTIME_SERVICE.to_owned();
        }
    }
}
fn migrate_legacy_ducc_config(document: &mut serde_json::Value) {
    let Some(providers) = document
        .get_mut("providers")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for provider in providers {
        let Some(request_policy) = provider
            .get_mut("request_policy")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        let legacy_bridge = request_policy
            .get("baidu_auth_bridge")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        if matches!(legacy_bridge.as_deref(), Some("ducc_loopback")) {
            request_policy.insert(
                "baidu_auth_bridge".to_owned(),
                serde_json::Value::String("disabled".to_owned()),
            );
            request_policy.insert(
                "baidu_code_report".to_owned(),
                serde_json::Value::Bool(false),
            );
            request_policy.remove("ducc_executable");
            request_policy.remove("data_report_executable");
            request_policy.remove("data_report_client_token");
        }
    }
}
fn backfill_data_report_executable(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if !provider.request_policy.baidu_code_report
            || provider.request_policy.data_report_executable.is_some()
        {
            continue;
        }
        provider.request_policy.data_report_executable = provider
            .request_policy
            .ducx_executable
            .as_ref()
            .and_then(|executable| {
                let install = executable.parent()?.parent()?;
                Some(install.join(crate::provider::auth::ducx::data_report_relative_path()))
            });
    }
}
/// A broken plugin-provided provider must not take the gateway down. Drop it
/// while loading so every reader and the next save see only valid providers;
/// the next Pi plugin sync recreates providers that are still installed.
fn drop_invalid_pi_plugin_providers(config: &mut StoredGatewayConfig) {
    let mut dropped = Vec::new();
    let mut index = 0;
    while index < config.providers.len() {
        let provider = &config.providers[index];
        if !is_managed_pi_provider(provider) {
            index += 1;
            continue;
        }
        match ProviderRegistry::new(vec![provider.clone()]) {
            Ok(_) => index += 1,
            Err(error) => {
                tracing::warn!(
                    provider = %provider.id,
                    error = %format!("{error:#}"),
                    "skipping invalid Pi plugin provider"
                );
                dropped.push(config.providers.remove(index));
            }
        }
    }
    // Plugin providers can also conflict with other providers (duplicate
    // routes, a second thread-title model). Drop the most recently added
    // plugin provider until the remaining providers load together.
    while let Err(error) = ProviderRegistry::new(config.providers.clone()) {
        let Some(index) = config.providers.iter().rposition(is_managed_pi_provider) else {
            break;
        };
        let provider = config.providers.remove(index);
        tracing::warn!(
            provider = %provider.id,
            error = %format!("{error:#}"),
            "skipping Pi plugin provider that conflicts with configured providers"
        );
        dropped.push(provider);
    }
    if dropped.is_empty() {
        return;
    }
    // Keep one disabled entry when plugin providers were the only providers,
    // matching how a Pi sync retires the last provider.
    if config.providers.is_empty() {
        // Rebuild from scratch so no field of the invalid entry survives.
        let source = &dropped[0];
        let mut provider = ProviderPreset::Custom.create(&source.id, "");
        provider.display_name = source.display_name.clone();
        provider.base_url = crate::provider::PI_BRIDGE_PLACEHOLDER_URL.to_owned();
        provider.api_path = "/v1/chat/completions".to_owned();
        provider.protocol = ProviderProtocol::OpenAiChat;
        provider.model_source = ProviderModelSource::Static;
        provider.pi_provider_id = source.pi_provider_id.clone();
        provider.pi_plugin_package = source.pi_plugin_package.clone();
        provider.enabled = false;
        match ProviderRegistry::new(vec![provider.clone()]) {
            Ok(_) => config.providers.push(provider),
            Err(error) => tracing::warn!(
                provider = %provider.id,
                error = %format!("{error:#}"),
                "failed to keep a disabled Pi plugin provider placeholder"
            ),
        }
    }
    // Routes that referenced the dropped models would otherwise fail every
    // later load and save.
    let Ok(providers) = ProviderRegistry::new(config.providers.clone()) else {
        return;
    };
    let mut unavailable_routes = Vec::new();
    for profile in &config.fusion_profiles {
        if validate_fusion_model_references(std::slice::from_ref(profile), &providers).is_err() {
            unavailable_routes.push(profile.model_slug());
        }
    }
    for route in &config.failover_routes {
        if validate_failover_model_references(std::slice::from_ref(route), &providers).is_err() {
            unavailable_routes.push(route.entry_model());
        }
    }
    for route in unavailable_routes {
        if !config.disabled_routes.contains(&route) {
            tracing::warn!(
                route = %route,
                "model route disabled because referenced Pi plugin provider was skipped"
            );
            config.disabled_routes.push(route);
        }
    }
}
fn upgrade_deepseek_quota_defaults(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() == Some("deepseek")
            && provider.base_url == "https://api.deepseek.com"
            && provider.quota_url.is_none()
            && provider.quota_parser == ProviderQuotaParser::Generic
        {
            provider.quota_url = Some("https://api.deepseek.com/user/balance".to_owned());
            provider.quota_parser = ProviderQuotaParser::DeepSeek;
        }
    }
}
fn upgrade_opencode_go_quota_defaults(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() == Some("opencode-go")
            && provider.base_url == "https://opencode.ai/zen/go"
            && provider.quota_url.is_none()
            && provider.quota_parser == ProviderQuotaParser::Generic
        {
            provider.quota_parser = ProviderQuotaParser::OpenCodeGo;
            provider.quota_currency = Some("USD".to_owned());
        }
    }
}
/// Move DeepSeek providers onto the Responses endpoint.
///
/// The preset used chat completions, which drops native reasoning and rejects
/// the Codex tool shapes. Only rewrite the untouched preset default; a
/// customized endpoint stays as the user's own choice.
fn upgrade_deepseek_responses_endpoint(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() == Some("deepseek")
            && provider.base_url == "https://api.deepseek.com"
            && provider.protocol == ProviderProtocol::OpenAiChat
            && matches!(
                provider.api_path.as_str(),
                "/chat/completions" | "/v1/chat/completions"
            )
        {
            provider.protocol = ProviderProtocol::OpenAiResponses;
            provider.api_path = "/v1/responses".to_owned();
        }
    }
}
fn upgrade_opencode_go_transport(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() != Some(crate::provider::OPEN_CODE_GO_PRESET_ID) {
            continue;
        }
        provider
            .request_policy
            .session_affinity_header
            .get_or_insert_with(|| crate::provider::OPENCODE_SESSION_HEADER.to_owned());
        if provider.base_url == "https://opencode.ai/zen/go"
            && provider.protocol == ProviderProtocol::OpenAiChat
            && provider.api_path == "/v1/chat/completions"
        {
            provider.protocol = ProviderProtocol::OpenAiResponses;
            provider.api_path = "/v1/responses".to_owned();
        }
    }
}
fn upgrade_agy_endpoint(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() != Some("agy") {
            continue;
        }
        if provider.base_url == "https://generativelanguage.googleapis.com" {
            provider.base_url = "https://daily-cloudcode-pa.googleapis.com".to_owned();
            provider.api_path = "/v1internal:streamGenerateContent".to_owned();
        }
    }
}

fn bootstrap_unrefreshed_selected_models(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.models_refreshed_at_ms.is_none()
            && provider.cached_models.is_empty()
            && !provider.selected_models.is_empty()
        {
            provider.cached_models = provider
                .selected_models
                .iter()
                .map(|id| ProviderModel {
                    id: id.clone(),
                    ..ProviderModel::default()
                })
                .collect();
        }
    }
}
fn migrate_legacy_config(legacy: LegacyStoredGatewayConfig) -> anyhow::Result<StoredGatewayConfig> {
    let preset = ProviderPreset::parse(legacy.provider_preset.as_deref().unwrap_or("custom"))?;
    let mut provider = preset.create(
        preset.default_id(),
        legacy.upstream_api_key.unwrap_or_default(),
    );
    if let Some(base_url) = legacy.upstream_base_url {
        let mut base_url = base_url.trim().trim_end_matches('/').to_owned();
        if preset == ProviderPreset::BaiduOneApi {
            base_url = base_url.strip_suffix("/v1").unwrap_or(&base_url).to_owned();
        }
        provider.base_url = base_url;
    }
    if let Some(kind) = legacy.upstream_kind {
        provider.protocol = match kind.as_str() {
            "anthropic_messages" | "anthropic-messages" | "anthropic" => {
                ProviderProtocol::AnthropicMessages
            }
            "openai_chat" | "openai-chat" | "chat_completions" | "chat-completions" => {
                ProviderProtocol::OpenAiChat
            }
            other => anyhow::bail!("unsupported legacy upstream kind: {other}"),
        };
    }
    if let Some(path) = legacy.upstream_messages_path {
        provider.api_path = normalize_legacy_path(path);
    }
    if preset != ProviderPreset::BaiduOneApi
        && let Some(path) = legacy.upstream_models_path
    {
        provider.model_source = ProviderModelSource::OpenAiCompatible {
            path: normalize_legacy_path(path),
        };
    }
    provider.image_generation_path = legacy
        .upstream_image_generation_path
        .filter(|path| !path.trim().is_empty())
        .map(normalize_legacy_path);
    if let Some(quota_url) = legacy.quota_url.filter(|url| !url.trim().is_empty()) {
        provider.quota_url = Some(quota_url.trim().to_owned());
    }
    provider.quota_username = legacy
        .quota_username
        .filter(|username| !username.trim().is_empty())
        .map(|username| username.trim().to_owned());
    if provider.quota_parser == ProviderQuotaParser::BaiduOneApi
        && provider.quota_username.is_none()
    {
        provider.quota_url = None;
    }
    provider.enabled = provider.auth.is_configured() && !provider.base_url.trim().is_empty();
    Ok(StoredGatewayConfig {
        context: Default::default(),
        config_version: CONFIG_VERSION,
        gateway_bind: legacy.gateway_bind,
        gateway_api_key: legacy.gateway_api_key,
        gateway_api_keys: Vec::new(),
        cors_allowed_origins: super::cors::default_cors_origins(),
        gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
        compaction_secret: None,
        official_ech_fallback_reason: None,
        official_ech_proxy: false,
        official_selected_models: None,
        official_model_order: Default::default(),
        official_model_contexts: Default::default(),
        fusion_profiles: legacy.fusion_profiles,
        failover_routes: Vec::new(),
        disabled_routes: Vec::new(),
        route_order: Vec::new(),
        providers: vec![provider],
    })
}
fn normalize_legacy_path(path: String) -> String {
    let path = path.trim();
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}
