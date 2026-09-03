use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use codex_mixin::config::{
    GatewayConfig, StoredGatewayConfig, load_stored_config, mutate_stored_config,
};
use codex_mixin::provider::{
    BaiduAuthBridge, ProviderDefinition, ProviderModel, ProviderProtocol, ProviderQuotaParser,
    spec_for,
};
use codex_mixin::provider_capabilities::ProviderCapabilities;
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
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::Router;
    use axum::http::{HeaderMap, header};
    use axum::routing::get;
    use codex_mixin::fusion::{FusionProfile, PanelToolsConfig};

    use super::discovery::{
        detect_custom_provider_protocol, discover_custom_quota, endpoint_join,
        infer_custom_provider_endpoint, protocol_probe_body_matches,
    };
    use super::management::{
        remove_provider_from_config, reorder_provider_ids, set_auxiliary_model_upstream,
    };
    use super::models::apply_model_selection;
    use super::*;
    use codex_mixin::provider::{ProviderModel, redact_provider_error};

    #[test]
    fn selecting_unknown_model_adds_it_with_safe_defaults() {
        let mut provider = codex_mixin::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();

        let contexts = BTreeMap::from([("hidden-model".to_owned(), 256_000)]);
        apply_model_selection(&mut provider, vec!["hidden-model".to_owned()], &contexts).unwrap();

        assert_eq!(provider.selected_models, ["hidden-model"]);
        let model = &provider.cached_models[0];
        assert!(model.manually_added);
        assert_eq!(model.context_window, Some(256_000));
        assert_eq!(model.supports_image, Some(false));
        assert_eq!(model.supports_thinking, Some(true));
        assert_eq!(model.supports_web_search, Some(false));
        assert_eq!(model.supports_tool_search, Some(false));
        assert_eq!(model.supports_function_tools, Some(true));

        apply_model_selection(&mut provider, Vec::new(), &BTreeMap::new()).unwrap();

        assert!(provider.selected_models.is_empty());
        assert!(provider.cached_models.is_empty());
    }

    #[test]
    fn context_override_rejects_discovered_model() {
        let mut provider = codex_mixin::provider::custom_provider("custom", "key");
        provider
            .cached_models
            .push(codex_mixin::provider::ProviderModel {
                id: "discovered-model".to_owned(),
                ..codex_mixin::provider::ProviderModel::default()
            });
        let contexts = BTreeMap::from([("discovered-model".to_owned(), 256_000)]);

        let error = apply_model_selection(
            &mut provider,
            vec!["discovered-model".to_owned()],
            &contexts,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("model context can only be edited for manually added models")
        );
    }

    #[test]
    fn official_provider_view_is_reserved_and_read_only() {
        let config = StoredGatewayConfig {
            official_selected_models: Some(vec!["gpt-5.6-sol".to_owned()]),
            ..StoredGatewayConfig::default()
        };
        let provider = official_provider_view(
            &config,
            vec![ProviderModel {
                id: "gpt-5.6-sol".to_owned(),
                ..ProviderModel::default()
            }],
        );

        assert!(official_provider_is_available(Some("codex_oauth_proxy")));
        assert!(!official_provider_is_available(Some("custom_only")));
        assert!(!official_provider_is_available(None));
        assert_eq!(provider["id"], "official");
        assert_eq!(provider["kind"], "official");
        assert_eq!(provider["display_name"], "OpenAI");
        assert_eq!(provider["enabled"], true);
        assert_eq!(provider["selected_models"], json!(["gpt-5.6-sol"]));
        assert_eq!(provider["cached_models"][0]["id"], "gpt-5.6-sol");
    }

    #[test]
    fn baidu_oneapi_add_without_bridge_leaves_loopback_unset() {
        let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");

        apply_baidu_auth_options(&mut provider, None, None).unwrap();

        assert_eq!(provider.request_policy.baidu_auth_bridge, None);
    }

    #[test]
    fn provider_mutations_persist_managed_ducx_options() {
        let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");
        provider.quota_username = Some("user@example.com".to_owned());
        let executable =
            PathBuf::from("/Users/example/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx");

        apply_baidu_auth_options(
            &mut provider,
            Some("ducx_loopback"),
            Some(executable.clone()),
        )
        .unwrap();

        assert_eq!(
            provider.request_policy.baidu_auth_bridge,
            Some(BaiduAuthBridge::DucxLoopback)
        );
        assert_eq!(provider.request_policy.ducx_executable, Some(executable));
        provider.request_policy.baidu_code_report = true;
        provider.request_policy.data_report_executable = provider
            .request_policy
            .ducx_executable
            .as_deref()
            .and_then(data_report_sibling);
        assert_eq!(
            provider.request_policy.data_report_executable,
            Some(PathBuf::from(
                "/Users/example/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/hooks/data-report"
            ))
        );
        provider.validate().unwrap();
    }

    #[test]
    fn parses_custom_header_environment_mappings() {
        let mapping = parse_header_env(&[
            "x-example-auth=EXAMPLE_AUTH".to_owned(),
            "x-routing-token=ROUTING_TOKEN".to_owned(),
        ])
        .unwrap();

        assert_eq!(mapping["x-example-auth"], "EXAMPLE_AUTH");
        assert_eq!(mapping["x-routing-token"], "ROUTING_TOKEN");
        assert!(parse_header_env(&["missing-separator".to_owned()]).is_err());
    }

    #[test]
    fn parses_opencode_go_quota_parser() {
        assert_eq!(
            parse_quota_parser("opencode_go").unwrap(),
            ProviderQuotaParser::OpenCodeGo
        );
        assert_eq!(
            parse_quota_parser("opencode-go").unwrap(),
            ProviderQuotaParser::OpenCodeGo
        );
    }

    #[tokio::test]
    async fn discovers_a_read_only_custom_quota_endpoint() {
        let authorization = Arc::new(Mutex::new(None));
        let captured_authorization = Arc::clone(&authorization);
        let app = Router::new().route(
            "/api/v1/credits",
            get(move |headers: HeaderMap| {
                let captured_authorization = Arc::clone(&captured_authorization);
                async move {
                    *captured_authorization.lock().unwrap() = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    axum::Json(serde_json::json!({
                        "data": {
                            "total_usage": 12.5,
                            "total_credits": 100,
                            "currency": "USD"
                        }
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "community-secret");
        provider.base_url = format!("http://{address}");
        let client = reqwest::Client::new();

        let discovered = discover_custom_quota(&client, &provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            discovered.url.as_str(),
            format!("http://{address}/api/v1/credits")
        );
        assert_eq!(discovered.currency.as_deref(), Some("USD"));
        assert_eq!(discovered.usage.used, Some(12.5));
        assert_eq!(discovered.usage.limit, Some(100.0));
        assert_eq!(
            authorization.lock().unwrap().as_deref(),
            Some("Bearer community-secret")
        );
    }

    #[tokio::test]
    async fn discovers_new_api_token_usage_with_its_canonical_trailing_slash() {
        let app = Router::new().route(
            "/api/usage/token/",
            get(|| async {
                axum::Json(serde_json::json!({
                    "code": true,
                    "message": "ok",
                    "data": {
                        "object": "token_usage",
                        "total_granted": 100,
                        "total_used": 12.5,
                        "total_available": 87.5
                    }
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("new-api", "new-api-key");
        provider.base_url = format!("http://{address}");

        let discovered = discover_custom_quota(&reqwest::Client::new(), &provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            discovered.url.as_str(),
            format!("http://{address}/api/usage/token/")
        );
        assert_eq!(discovered.usage.used, Some(12.5));
        assert_eq!(discovered.usage.limit, Some(100.0));
        assert_eq!(discovered.usage.remaining, Some(87.5));
    }

    #[tokio::test]
    async fn discovers_sub2api_wallet_usage_from_the_api_key_endpoint() {
        let app = Router::new().route(
            "/v1/usage",
            get(|| async {
                axum::Json(serde_json::json!({
                    "mode": "unrestricted",
                    "isValid": true,
                    "remaining": 37.5,
                    "balance": 37.5,
                    "unit": "USD",
                    "usage": {
                        "total": {
                            "actual_cost": 12.5
                        }
                    }
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("sub2api", "sub2api-key");
        provider.base_url = format!("http://{address}");

        let discovered = discover_custom_quota(&reqwest::Client::new(), &provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            discovered.url.as_str(),
            format!("http://{address}/v1/usage")
        );
        assert_eq!(discovered.currency.as_deref(), Some("USD"));
        assert_eq!(discovered.usage.used, Some(12.5));
        assert_eq!(discovered.usage.limit, Some(50.0));
        assert_eq!(discovered.usage.remaining, Some(37.5));
    }

    #[test]
    fn reorders_provider_ids_without_changing_provider_data() {
        let mut first = codex_mixin::provider::custom_provider("first", "first-key");
        first.selected_models = vec!["first-model".to_owned()];
        first.enabled = false;
        first.quota_username = Some("first-user".to_owned());
        first.request_policy.baidu_auth_bridge = Some(BaiduAuthBridge::DucxLoopback);
        first.request_policy.baidu_code_report = true;
        let mut second = codex_mixin::provider::custom_provider("second", "second-key");
        second.selected_models = vec!["second-model".to_owned()];
        second.quota_username = Some("second-user".to_owned());
        let first_before = first.clone();
        let second_before = second.clone();
        let mut config = StoredGatewayConfig {
            providers: vec![first, second],
            ..StoredGatewayConfig::default()
        };

        reorder_provider_ids(&mut config, &["second".to_owned(), "first".to_owned()]).unwrap();

        assert_eq!(
            config
                .providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(config.providers[0].selected_models, ["second-model"]);
        assert_eq!(config.providers[1].selected_models, ["first-model"]);
        assert_eq!(config.providers, [second_before, first_before]);
    }

    #[test]
    fn rejects_incomplete_or_duplicate_provider_orders() {
        let first = codex_mixin::provider::custom_provider("first", "first-key");
        let second = codex_mixin::provider::custom_provider("second", "second-key");
        let mut config = StoredGatewayConfig {
            providers: vec![first, second],
            ..StoredGatewayConfig::default()
        };

        assert!(reorder_provider_ids(&mut config, &["first".to_owned()]).is_err());
        assert!(
            reorder_provider_ids(&mut config, &["first".to_owned(), "first".to_owned()]).is_err()
        );
        assert_eq!(
            config
                .providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn removing_a_generated_provider_compacts_ids_and_fusion_model_references() {
        let mut first = codex_mixin::provider::custom_provider("custom", "first-key");
        first.selected_models = vec!["first-model".to_owned()];
        first.cached_models = vec![ProviderModel {
            id: "first-model".to_owned(),
            ..ProviderModel::default()
        }];
        let mut second = codex_mixin::provider::custom_provider("custom-2", "second-key");
        second.selected_models = vec!["second-model".to_owned()];
        second.cached_models = vec![ProviderModel {
            id: "second-model".to_owned(),
            ..ProviderModel::default()
        }];
        let mut third = codex_mixin::provider::custom_provider("custom-3", "third-key");
        third.selected_models = vec!["third-model".to_owned()];
        third.cached_models = vec![ProviderModel {
            id: "third-model".to_owned(),
            ..ProviderModel::default()
        }];
        let mut config = StoredGatewayConfig {
            providers: vec![first, second, third],
            fusion_profiles: vec![FusionProfile {
                id: "review".to_owned(),
                panel_models: vec![
                    "second-model-custom-2".to_owned(),
                    "third-model-custom-3".to_owned(),
                ],
                judge_model: "second-model-custom-2".to_owned(),
                final_model: "third-model-custom-3".to_owned(),
                min_successful: 1,
                max_completion_tokens: 2_048,
                timeout_ms: 300_000,
                show_intermediate_results: true,
                panel_tools: PanelToolsConfig::default(),
            }],
            ..StoredGatewayConfig::default()
        };

        remove_provider_from_config(&mut config, "custom").unwrap();

        assert_eq!(
            config
                .providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            ["custom", "custom-2"]
        );
        assert_eq!(
            config.fusion_profiles[0].panel_models,
            ["second-model-custom", "third-model-custom-2"]
        );
        assert_eq!(config.fusion_profiles[0].judge_model, "second-model-custom");
        assert_eq!(
            config.fusion_profiles[0].final_model,
            "third-model-custom-2"
        );
    }

    #[test]
    fn selecting_auxiliary_model_upstream_is_exclusive_and_can_be_cleared() {
        let first = codex_mixin::provider::custom_provider("first", "first-key");
        let mut second = codex_mixin::provider::custom_provider("second", "second-key");
        second.auxiliary_model_upstream = true;
        let mut config = StoredGatewayConfig {
            providers: vec![first, second],
            ..StoredGatewayConfig::default()
        };

        set_auxiliary_model_upstream(&mut config, "first", true).unwrap();
        assert!(config.providers[0].auxiliary_model_upstream);
        assert!(!config.providers[1].auxiliary_model_upstream);

        set_auxiliary_model_upstream(&mut config, "first", false).unwrap();
        assert!(
            config
                .providers
                .iter()
                .all(|provider| !provider.auxiliary_model_upstream)
        );
    }

    #[test]
    fn infers_custom_provider_endpoints_without_exposing_protocol_fields() {
        let openai = infer_custom_provider_endpoint("https://public.example/v1").unwrap();
        assert_eq!(openai.base_url, "https://public.example/v1");
        assert_eq!(openai.protocol, ProviderProtocol::OpenAiResponses);
        assert_eq!(openai.api_path, "/v1/responses");
        assert_eq!(openai.models_path, "/v1/models");
        assert!(!openai.path_explicit);

        let anthropic =
            infer_custom_provider_endpoint("https://public.example/api/v1/messages").unwrap();
        assert_eq!(anthropic.base_url, "https://public.example/api");
        assert_eq!(anthropic.protocol, ProviderProtocol::AnthropicMessages);
        assert_eq!(anthropic.api_path, "/v1/messages");
        assert_eq!(anthropic.models_path, "/v1/models");
        assert!(anthropic.path_explicit);

        let responses =
            infer_custom_provider_endpoint("https://public.example/v1/responses").unwrap();
        assert_eq!(responses.base_url, "https://public.example");
        assert_eq!(responses.protocol, ProviderProtocol::OpenAiResponses);
        assert_eq!(responses.api_path, "/v1/responses");
        assert_eq!(responses.models_path, "/v1/models");
        assert!(responses.path_explicit);
        assert_eq!(
            endpoint_join("https://public.example/api/v1", "/v1/models")
                .unwrap()
                .as_str(),
            "https://public.example/api/v1/models"
        );
    }

    #[tokio::test]
    async fn detects_responses_before_messages_and_chat_for_custom_providers() {
        use axum::routing::post;
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
            )
            .route(
                "/v1/responses",
                post(|| async {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                    )
                }),
            )
            .route(
                "/v1/messages",
                post(|| async {
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({"id":"messages-should-not-win"})),
                    )
                }),
            )
            .route(
                "/v1/chat/completions",
                post(|| async {
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({"id":"should-not-win"})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");
        assert_eq!(
            endpoint_join(&provider.base_url, "/v1/models")
                .unwrap()
                .path(),
            "/v1/models"
        );

        let detected = detect_custom_provider_protocol(&provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detected.protocol, ProviderProtocol::OpenAiResponses);
        assert_eq!(detected.api_path, "/v1/responses");
        assert_eq!(detected.models_path, "/v1/models");
    }

    #[tokio::test]
    async fn allows_slow_custom_protocol_probes() {
        use axum::routing::post;
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
            )
            .route(
                "/v1/responses",
                post(|| async {
                    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let detected = detect_custom_provider_protocol(&provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detected.protocol, ProviderProtocol::OpenAiResponses);
    }

    #[tokio::test]
    async fn forbidden_protocol_probes_do_not_switch_custom_providers_to_messages() {
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
            )
            .fallback(|| async {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({
                        "error": {"message": "This group does not allow this protocol dispatch"}
                    })),
                )
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let error = detect_custom_provider_protocol(&provider)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("models endpoint is valid"));
        assert!(error.contains("protocol detection failed"));
        assert!(error.contains("within 30 seconds"));
        assert_eq!(provider.protocol, ProviderProtocol::OpenAiResponses);
        assert_eq!(provider.api_path, "/v1/responses");
    }

    #[tokio::test]
    async fn falls_back_to_messages_when_responses_is_missing() {
        use axum::routing::post;
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
            )
            .route(
                "/v1/messages",
                post(|| async {
                    (
                        axum::http::StatusCode::UNAUTHORIZED,
                        axum::Json(serde_json::json!({"error":{"type":"authentication_error"}})),
                    )
                }),
            )
            .route(
                "/v1/chat/completions",
                post(|| async {
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({"id":"chat"})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let detected = detect_custom_provider_protocol(&provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detected.protocol, ProviderProtocol::AnthropicMessages);
        assert_eq!(detected.api_path, "/v1/messages");
    }

    #[tokio::test]
    async fn falls_back_to_chat_when_native_apis_are_missing() {
        use axum::routing::post;
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
            )
            .route(
                "/v1/chat/completions",
                post(|| async {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"error":{"message":"missing messages"}})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let detected = detect_custom_provider_protocol(&provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detected.protocol, ProviderProtocol::OpenAiChat);
        assert_eq!(detected.api_path, "/v1/chat/completions");
    }

    #[tokio::test]
    async fn accepts_an_empty_v1_models_list_as_a_real_endpoint() {
        use axum::routing::post;
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::Json(serde_json::json!({"data":[]})) }),
            )
            .route(
                "/v1/responses",
                post(|| async {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let detected = detect_custom_provider_protocol(&provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detected.protocol, ProviderProtocol::OpenAiResponses);
    }

    #[tokio::test]
    async fn falls_back_to_legacy_paths_after_v1_models_failure() {
        use axum::response::Html;
        use axum::routing::post;
        let legacy_requests = Arc::new(AtomicUsize::new(0));
        let legacy_requests_for_handler = Arc::clone(&legacy_requests);
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { Html("<!doctype html><html>login</html>") }),
            )
            .route(
                "/models",
                get(move || {
                    legacy_requests_for_handler.fetch_add(1, Ordering::Relaxed);
                    async { axum::Json(serde_json::json!({"data":[{"id":"legacy"}]})) }
                }),
            )
            .route(
                "/responses",
                post(|| async {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let detected = detect_custom_provider_protocol(&provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detected.api_path, "/responses");
        assert_eq!(detected.models_path, "/models");
        assert_eq!(legacy_requests.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn falls_back_to_legacy_response_paths_after_v1_models_succeeds() {
        use axum::routing::post;
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
            )
            .route(
                "/models",
                get(|| async { axum::Json(serde_json::json!({"data":[{"id":"legacy"}]})) }),
            )
            .route(
                "/responses",
                post(|| async {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let detected = detect_custom_provider_protocol(&provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detected.api_path, "/responses");
        assert_eq!(detected.models_path, "/models");
    }

    #[tokio::test]
    async fn falls_back_to_legacy_paths_after_v1_models_api_error() {
        use axum::routing::post;
        let legacy_requests = Arc::new(AtomicUsize::new(0));
        let legacy_requests_for_handler = Arc::clone(&legacy_requests);
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async {
                    (
                        axum::http::StatusCode::UNAUTHORIZED,
                        axum::Json(serde_json::json!({
                            "error": {"message": "invalid API key"}
                        })),
                    )
                }),
            )
            .route(
                "/models",
                get(move || {
                    legacy_requests_for_handler.fetch_add(1, Ordering::Relaxed);
                    async { axum::Json(serde_json::json!({"data": [{"id": "legacy"}]})) }
                }),
            )
            .route(
                "/responses",
                post(|| async {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let detected = detect_custom_provider_protocol(&provider)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(detected.api_path, "/responses");
        assert_eq!(detected.models_path, "/models");
        assert_eq!(legacy_requests.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn rejects_unrelated_json_from_the_models_endpoint() {
        let app = Router::new().route(
            "/v1/models",
            get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = codex_mixin::provider::custom_provider("community", "secret");
        provider.base_url = format!("http://{address}");

        let error = detect_custom_provider_protocol(&provider)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("neither a valid /v1/models nor /models endpoint"));
    }

    #[test]
    fn protocol_probe_rejects_pages_and_accepts_protocol_errors() {
        assert!(!protocol_probe_body_matches(
            ProviderProtocol::OpenAiResponses,
            "text/html; charset=utf-8",
            "<html>dashboard</html>"
        ));
        assert!(!protocol_probe_body_matches(
            ProviderProtocol::OpenAiResponses,
            "application/json",
            "{\"object\":\"list\",\"data\":[]}"
        ));
        assert!(protocol_probe_body_matches(
            ProviderProtocol::OpenAiResponses,
            "application/json",
            "{\"error\":{\"message\":\"missing input\"}}"
        ));
        assert!(protocol_probe_body_matches(
            ProviderProtocol::OpenAiResponses,
            "text/event-stream",
            "data: {\"id\":\"resp_1\",\"object\":\"response\"}\n\n"
        ));
        assert!(protocol_probe_body_matches(
            ProviderProtocol::OpenAiResponses,
            "text/event-stream",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n"
        ));
        assert!(protocol_probe_body_matches(
            ProviderProtocol::AnthropicMessages,
            "text/event-stream",
            "data: {\"type\":\"message_start\",\"message\":{\"type\":\"message\",\"content\":[]}}\n\n"
        ));
    }

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
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(provider.selected_models, ["glm-5.2", "temporarily-gone"]);
        assert!(provider.new_models.is_empty());

        apply_model_selection(&mut provider, vec!["glm-5.2".to_owned()], &BTreeMap::new()).unwrap();
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
