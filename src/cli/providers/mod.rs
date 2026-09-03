use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use codex_mixin::config::{
    GatewayConfig, StoredGatewayConfig, load_stored_config, mutate_stored_config,
};
use codex_mixin::provider::capabilities::ProviderCapabilities;
use codex_mixin::provider::{
    BaiduAuthBridge, ProviderDefinition, ProviderModel, ProviderProtocol, ProviderQuotaParser,
    spec_for,
};
use codex_mixin::web_search::WebSearchCapabilities;
use console::style;
use serde_json::json;

use super::codex::{
    codex_home_path, managed_codex_install_mode, reconcile_managed_skills,
    resolve_codex_config_path,
};
use super::config_input::{normalize_base_url, trim_required};
use super::official_models::load_official_models;
mod discovery;
mod management;
mod models;
pub(super) use management::{
    add_provider, remove_provider, reorder_providers, set_provider_enabled, update_provider,
};
pub(super) use models::{
    discover_models, discover_models_with_output, probe_selected_models, select_models,
    test_provider,
};

#[derive(Clone, Debug)]
pub(super) struct AddProviderOptions {
    pub(super) preset: String,
    pub(super) auxiliary_model_upstream: Option<bool>,
    pub(super) id: Option<String>,
    pub(super) key: Option<String>,
    pub(super) aws_access_key_id: Option<String>,
    pub(super) aws_secret_access_key: Option<String>,
    pub(super) aws_session_token: Option<String>,
    pub(super) aws_region: Option<String>,
    pub(super) display_name: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) website_url: Option<String>,
    pub(super) protocol: Option<String>,
    pub(super) api_path: Option<String>,
    pub(super) models_path: Option<String>,
    pub(super) image_generation_path: Option<String>,
    pub(super) quota_url: Option<String>,
    pub(super) quota_username: Option<String>,
    pub(super) quota_workspace_id: Option<String>,
    pub(super) quota_auth_cookie: Option<String>,
    pub(super) quota_currency: Option<String>,
    pub(super) quota_parser: Option<String>,
    pub(super) gateway_key: Option<String>,
    pub(super) static_models: Vec<String>,
    pub(super) header_env: Vec<String>,
    pub(super) baidu_auth_bridge: Option<String>,
    pub(super) ducx_executable: Option<PathBuf>,
    pub(super) baidu_code_report: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct UpdateProviderOptions {
    pub(super) id: String,
    pub(super) auxiliary_model_upstream: Option<bool>,
    pub(super) key: Option<String>,
    pub(super) clear_key: bool,
    pub(super) aws_access_key_id: Option<String>,
    pub(super) aws_secret_access_key: Option<String>,
    pub(super) aws_session_token: Option<String>,
    pub(super) aws_region: Option<String>,
    pub(super) clear_aws_session_token: bool,
    pub(super) clear_aws_credentials: bool,
    pub(super) display_name: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) website_url: Option<String>,
    pub(super) protocol: Option<String>,
    pub(super) api_path: Option<String>,
    pub(super) models_path: Option<String>,
    pub(super) image_generation_path: Option<String>,
    pub(super) clear_image_generation: bool,
    pub(super) quota_url: Option<String>,
    pub(super) clear_quota: bool,
    pub(super) quota_username: Option<String>,
    pub(super) quota_workspace_id: Option<String>,
    pub(super) clear_quota_workspace_id: bool,
    pub(super) quota_auth_cookie: Option<String>,
    pub(super) clear_quota_auth_cookie: bool,
    pub(super) quota_currency: Option<String>,
    pub(super) quota_parser: Option<String>,
    pub(super) header_env: Vec<String>,
    pub(super) clear_header_env: bool,
    pub(super) baidu_auth_bridge: Option<String>,
    pub(super) ducx_executable: Option<PathBuf>,
    pub(super) baidu_code_report: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct TestProviderOptions {
    pub(super) id: String,
    pub(super) json: bool,
    pub(super) key: Option<String>,
    pub(super) aws_access_key_id: Option<String>,
    pub(super) aws_secret_access_key: Option<String>,
    pub(super) aws_session_token: Option<String>,
    pub(super) aws_region: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) baidu_auth_bridge: Option<String>,
    pub(super) ducx_executable: Option<PathBuf>,
}

pub(super) fn list_providers(json_output: bool) -> anyhow::Result<()> {
    let mut config = load_stored_config()?.unwrap_or_default();
    let runtime_config = GatewayConfig::from_stored_config()?;
    let codex_install_mode = managed_codex_install_mode(&resolve_codex_config_path(None)?)?;
    let capabilities = ProviderCapabilities::from_default_path(&runtime_config)?;
    for provider in &mut config.providers {
        capabilities.annotate_provider(provider);
    }
    if json_output {
        let mut providers = config
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
                    "kind": "configured",
                    "icon": spec_for(provider.preset_id.as_deref()).icon,
                    "display_name": provider.display_name,
                    "enabled": provider.enabled,
                    "auxiliary_model_upstream": provider.auxiliary_model_upstream,
                    "preset_id": provider.preset_id,
                    "protocol": provider.protocol,
                    "base_url": provider.base_url,
                    "website_url": provider.website_url,
                    "api_path": provider.api_path,
                    "model_source": provider.model_source,
                    "api_key_configured": provider.auth.is_configured(),
                    "aws_sigv4_configured": provider.auth.aws_sigv4.is_some(),
                    "aws_region": provider.auth.aws_sigv4.as_ref().map(|aws| &aws.region),
                    "aws_session_token_configured": provider
                        .auth
                        .aws_sigv4
                        .as_ref()
                        .and_then(|aws| aws.session_token.as_deref())
                        .is_some_and(|token| !token.is_empty()),
                    "image_generation_path": provider.image_generation_path,
                    "quota_url": provider.quota_url,
                    "quota_username": provider.quota_username,
                    "quota_workspace_id": provider.quota_workspace_id,
                    "quota_auth_cookie_configured": provider
                        .quota_auth_cookie
                        .as_deref()
                        .is_some_and(|cookie| !cookie.is_empty()),
                    "quota_currency": provider.quota_currency,
                    "quota_parser": provider.quota_parser,
                    "custom_headers_from_env": provider.request_policy.custom_headers_from_env,
                    "baidu_auth_bridge": provider.request_policy.baidu_auth_bridge,
                    "baidu_code_report": provider.request_policy.baidu_code_report,
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
        if official_provider_is_available(codex_install_mode) {
            providers.insert(0, official_provider_view(&config, load_official_models()?));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "config_version": config.config_version,
                "gateway_bind": config.gateway_bind,
                "gateway_auth_configured": config.gateway_api_key.is_some(),
                "codex_install_mode": codex_install_mode,
                "fusion_profile": config.fusion_profiles.first(),
                "providers": providers,
            }))?
        );
        return Ok(());
    }
    if config.providers.is_empty() && !official_provider_is_available(codex_install_mode) {
        println!("no providers configured");
        return Ok(());
    }
    let mut table_rows: Vec<_> = config
        .providers
        .iter()
        .map(|provider| {
            let readiness = provider.readiness();
            (
                provider.id.clone(),
                provider.display_name.clone(),
                if provider.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                protocol_name(provider.protocol),
                format!(
                    "{}/{}",
                    provider.selected_models.len(),
                    provider.cached_models.len()
                ),
                readiness.routable_model_count.to_string(),
                readiness.status.as_str().to_owned(),
            )
        })
        .collect();
    if official_provider_is_available(codex_install_mode) {
        table_rows.insert(
            0,
            (
                "official".to_owned(),
                "OpenAI".to_owned(),
                "managed",
                "responses",
                "-".to_owned(),
                "-".to_owned(),
                "healthy".to_owned(),
            ),
        );
    }
    let id_width = table_rows.iter().map(|row| row.0.len()).max().unwrap_or(2);
    let name_width = table_rows.iter().map(|row| row.1.len()).max().unwrap_or(4);
    println!(
        "  {:<id_width$}  {:<name_width$}  {:<9}  {:<10}  {:<9}  {:<8}  STATUS",
        style("ID").bold(),
        style("NAME").bold(),
        style("STATE").bold(),
        style("PROTOCOL").bold(),
        style("SELECTED").bold(),
        style("ROUTABLE").bold(),
        id_width = id_width,
        name_width = name_width,
    );
    for (id, name, state, protocol, selected, routable, status) in &table_rows {
        let state_styled = if *state == "enabled" {
            style(*state).green()
        } else {
            style(*state).dim()
        };
        let status_styled = match status.as_str() {
            "healthy" => style(status).green(),
            "degraded" => style(status).yellow(),
            _ => style(status).red(),
        };
        println!(
            "  {:<id_width$}  {:<name_width$}  {:<9}  {:<10}  {:<9}  {:<8}  {}",
            id,
            name,
            state_styled,
            protocol,
            selected,
            routable,
            status_styled,
            id_width = id_width,
            name_width = name_width,
        );
    }
    Ok(())
}

fn official_provider_is_available(codex_install_mode: Option<&str>) -> bool {
    codex_install_mode == Some("codex_oauth_proxy")
}

fn official_provider_view(
    config: &StoredGatewayConfig,
    cached_models: Vec<ProviderModel>,
) -> serde_json::Value {
    let available_models = cached_models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<HashSet<_>>();
    let selected_models = config.official_selected_models.as_ref().map_or_else(
        || cached_models.iter().map(|model| model.id.clone()).collect(),
        |selected| {
            selected
                .iter()
                .filter(|model| available_models.contains(model.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        },
    );
    let routable_model_count = selected_models.len();
    json!({
        "id": "official",
        "kind": "official",
        "icon": "openai",
        "display_name": "OpenAI",
        "enabled": true,
        "auxiliary_model_upstream": false,
        "preset_id": null,
        "protocol": "open_ai_responses",
        "base_url": "https://chatgpt.com/backend-api/codex",
        "website_url": "https://chatgpt.com",
        "api_path": "/responses",
        "model_source": {"kind": "static"},
        "api_key_configured": true,
        "image_generation_path": null,
        "quota_url": null,
        "quota_username": null,
        "quota_workspace_id": null,
        "quota_auth_cookie_configured": false,
        "quota_currency": null,
        "quota_parser": "generic",
        "custom_headers_from_env": {},
        "baidu_auth_bridge": null,
        "baidu_code_report": false,
        "selected_models": selected_models,
        "new_models": [],
        "unavailable_selected_models": [],
        "cached_models": cached_models,
        "models_refreshed_at_ms": null,
        "last_model_refresh_error": null,
        "readiness": "healthy",
        "readiness_issues": [],
        "routable_model_count": routable_model_count,
    })
}

fn apply_baidu_auth_options(
    provider: &mut ProviderDefinition,
    bridge: Option<&str>,
    ducx_executable: Option<PathBuf>,
) -> anyhow::Result<()> {
    if let Some(bridge) = bridge {
        provider.request_policy.baidu_auth_bridge = Some(match bridge {
            "disabled" => BaiduAuthBridge::Disabled,
            "ducx_loopback" => BaiduAuthBridge::DucxLoopback,
            other => anyhow::bail!(
                "invalid Baidu auth bridge {other}; expected disabled or ducx_loopback"
            ),
        });
    }
    if let Some(executable) = ducx_executable {
        provider.request_policy.ducx_executable = Some(executable);
    }
    Ok(())
}

fn data_report_sibling(executable: &std::path::Path) -> Option<PathBuf> {
    let install = executable.parent()?.parent()?;
    Some(install.join("hooks/data-report"))
}

fn parse_header_env(values: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    values
        .iter()
        .map(|value| {
            let (header, variable) = value.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("custom header mapping must use NAME=ENV_VAR: {value}")
            })?;
            Ok((header.trim().to_owned(), variable.trim().to_owned()))
        })
        .collect()
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

fn sync_imagegen_skill() -> anyhow::Result<()> {
    let config = required_config()?;
    let auxiliary_provider_enabled = config
        .providers
        .iter()
        .any(|provider| provider.enabled && provider.auxiliary_model_upstream);
    if reconcile_managed_skills(&codex_home_path(), auxiliary_provider_enabled)? {
        println!(
            "codex imagegen skill: {}; restart Codex Desktop to reload skills",
            if auxiliary_provider_enabled {
                "installed"
            } else {
                "restored official skill"
            }
        );
    }
    Ok(())
}

fn mutate_and_invalidate_provider_capabilities<T>(
    mutation: impl FnOnce(&mut StoredGatewayConfig) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let result = mutate_and_invalidate(mutation)?;
    ProviderCapabilities::clear_default_cache()?;
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
        "deepseek" => Ok(ProviderQuotaParser::DeepSeek),
        "opencode_go" | "opencode-go" => Ok(ProviderQuotaParser::OpenCodeGo),
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
mod tests;
