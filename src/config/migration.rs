use super::StoredGatewayConfig;
use crate::config::ensure_config_version;
use crate::provider::CONFIG_VERSION;
use crate::provider::ProviderModel;
use crate::provider::ProviderModelSource;
use crate::provider::ProviderPreset;
use crate::provider::ProviderProtocol;
use crate::provider::ProviderQuotaParser;
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
    fusion_profiles: Vec<crate::fusion::FusionProfile>,
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
        upgrade_opencode_go_quota_defaults(&mut parsed);
        upgrade_opencode_go_responses_endpoint(&mut parsed);
        bootstrap_unrefreshed_selected_models(&mut parsed);
        backfill_data_report_executable(&mut parsed);
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
    upgrade_opencode_go_quota_defaults(&mut migrated);
    upgrade_opencode_go_responses_endpoint(&mut migrated);
    bootstrap_unrefreshed_selected_models(&mut migrated);
    backfill_data_report_executable(&mut migrated);
    Ok(migrated)
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
                Some(install.join("hooks/data-report"))
            });
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
fn upgrade_opencode_go_responses_endpoint(config: &mut StoredGatewayConfig) {
    for provider in &mut config.providers {
        if provider.preset_id.as_deref() == Some("opencode-go")
            && provider.base_url == "https://opencode.ai/zen/go"
            && provider.protocol == ProviderProtocol::OpenAiChat
            && provider.api_path == "/v1/chat/completions"
        {
            provider.protocol = ProviderProtocol::OpenAiResponses;
            provider.api_path = "/v1/responses".to_owned();
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
    provider.enabled =
        !provider.auth.api_key.trim().is_empty() && !provider.base_url.trim().is_empty();
    Ok(StoredGatewayConfig {
        config_version: CONFIG_VERSION,
        gateway_bind: legacy.gateway_bind,
        gateway_api_key: legacy.gateway_api_key,
        compaction_secret: None,
        fusion_profiles: legacy.fusion_profiles,
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
