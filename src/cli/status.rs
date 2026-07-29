use std::collections::HashSet;
use std::time::Duration;

use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};
use codex_mixin::provider::{
    ProviderDefinition, ProviderQuotaParser, ProviderReadinessStatus, ProviderRegistry,
};
use codex_mixin::server::AppState;

use super::ConfigScope;
use super::runtime::*;

pub(super) async fn status(json_output: bool) -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config()?;
    let metadata = load_daemon_metadata()?;
    let runtime = load_runtime_metadata()?;
    let daemon_status = match &metadata {
        Some(metadata) if pid_is_running(metadata.pid)? => "running",
        Some(_) => "stale",
        None => "not_started",
    };
    let runtime_running = runtime
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    let bind = if runtime_running {
        runtime.as_ref().expect("live runtime metadata").bind
    } else {
        metadata
            .as_ref()
            .map_or(config.bind, |metadata| metadata.bind)
    };
    let url = format!("http://{bind}/healthz");
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?
        .get(&url)
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => {
            let endpoint = format!("http://{bind}/v1");
            let readiness = provider_readiness_summary(&config.providers);
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "daemon": daemon_status,
                        "pid": metadata.as_ref().map(|metadata| metadata.pid),
                        "log": metadata.as_ref().map(|metadata| metadata.log_file.clone()),
                        "gateway": "running",
                        "gateway_version": if runtime_running {
                            runtime
                                .as_ref()
                                .and_then(|metadata| metadata.version.as_deref())
                                .unwrap_or("unknown")
                        } else {
                            "unknown"
                        },
                        "bind": bind,
                        "healthz": url,
                        "endpoint": endpoint,
                        "provider_readiness": readiness.0,
                        "provider_counts": {
                            "total": config.providers.len(),
                            "healthy": readiness.1,
                            "degraded": readiness.2,
                            "disabled": readiness.3,
                        },
                        "providers": provider_readiness_values(&config.providers),
                    }))?
                );
            } else {
                println!("daemon: {}", daemon_status.replace('_', " "));
                if let Some(metadata) = &metadata {
                    println!("pid: {}", metadata.pid);
                    println!("log: {}", metadata.log_file.display());
                }
                println!(
                    "gateway-version: {}",
                    if runtime_running {
                        runtime
                            .as_ref()
                            .and_then(|metadata| metadata.version.as_deref())
                            .unwrap_or("unknown")
                    } else {
                        "unknown"
                    }
                );
                println!("gateway: running");
                println!("healthz: {url}");
                println!("endpoint: {endpoint}");
                println!("provider-readiness: {}", readiness.0);
                println!(
                    "providers: {} total, {} healthy, {} degraded, {} disabled",
                    config.providers.len(),
                    readiness.1,
                    readiness.2,
                    readiness.3,
                );
                for issue in provider_readiness_issue_descriptions(&config.providers) {
                    println!("provider-issue: {issue}");
                }
            }
            Ok(())
        }
        Ok(response) => anyhow::bail!("gateway unhealthy: {} returned {}", url, response.status()),
        Err(err) => anyhow::bail!("gateway not running at {url}: {err}"),
    }
}

fn provider_readiness_summary(
    providers: &[ProviderDefinition],
) -> (&'static str, usize, usize, usize) {
    let mut healthy = 0;
    let mut degraded = 0;
    let mut disabled = 0;
    for provider in providers {
        match provider.readiness().status {
            ProviderReadinessStatus::Healthy => healthy += 1,
            ProviderReadinessStatus::Degraded => degraded += 1,
            ProviderReadinessStatus::Disabled => disabled += 1,
        }
    }
    let status = if degraded > 0 {
        "degraded"
    } else if healthy > 0 {
        "healthy"
    } else {
        "disabled"
    };
    (status, healthy, degraded, disabled)
}

fn provider_readiness_values(providers: &[ProviderDefinition]) -> Vec<serde_json::Value> {
    providers
        .iter()
        .map(|provider| {
            serde_json::json!({
                "id": provider.id,
                "display_name": provider.display_name,
                "enabled": provider.enabled,
                "protocol": provider.protocol,
                "readiness": provider.readiness(),
            })
        })
        .collect()
}

fn provider_readiness_issue_descriptions(providers: &[ProviderDefinition]) -> Vec<String> {
    let mut descriptions = Vec::new();
    for provider in providers {
        let readiness = provider.readiness();
        if readiness.status != ProviderReadinessStatus::Degraded {
            continue;
        }
        let provider_name = single_line(&provider.display_name);
        if provider.auth.api_key.trim().is_empty() {
            descriptions.push(format!("{provider_name}：未配置 API Key"));
        }
        let available_models = provider
            .cached_models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<HashSet<_>>();
        let unavailable_models = provider
            .selected_models
            .iter()
            .filter(|model| !available_models.contains(model.as_str()))
            .map(|model| single_line(model))
            .collect::<Vec<_>>();
        if unavailable_models.is_empty() {
            if readiness.routable_model_count == 0 {
                descriptions.push(format!("{provider_name}：没有已启用的可用模型"));
            }
        } else {
            descriptions.push(format!(
                "{provider_name}：模型 {} 当前不可达",
                unavailable_models.join("、")
            ));
        }
        if let Some(error) = provider.models_refresh_error.as_deref() {
            descriptions.push(format!(
                "{provider_name}：模型列表刷新失败：{}",
                single_line(error)
            ));
        }
    }
    descriptions
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod provider_readiness_tests {
    use super::provider_readiness_issue_descriptions;
    use codex_mixin::provider::{ProviderModel, custom_provider};

    #[test]
    fn provider_issue_descriptions_name_unavailable_models_and_refresh_errors() {
        let mut provider = custom_provider("baidu-oneapi", "secret");
        provider.display_name = "Baidu OneAPI".to_owned();
        provider.selected_models =
            vec!["available-model".to_owned(), "unreachable-model".to_owned()];
        provider.cached_models = vec![ProviderModel {
            id: "available-model".to_owned(),
            ..ProviderModel::default()
        }];
        provider.models_refresh_error = Some("request failed\nupstream returned 503".to_owned());

        assert_eq!(
            provider_readiness_issue_descriptions(&[provider]),
            vec![
                "Baidu OneAPI：模型 unreachable-model 当前不可达",
                "Baidu OneAPI：模型列表刷新失败：request failed upstream returned 503",
            ]
        );
    }
}

pub(super) async fn models(json_output: bool) -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config()?;
    let state = AppState::new(config)?;
    let models = state.fetch_models().await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&models)?);
    } else {
        for model in models {
            println!("{}", model.id);
        }
    }
    Ok(())
}

pub(super) async fn probe_web_search(force: bool, json_output: bool) -> anyhow::Result<()> {
    let config = GatewayConfig::from_stored_config()?;
    let state = AppState::new(config)?;
    let mut models = state.fetch_models().await?;
    let summary = state
        .probe_web_search_capabilities(&mut models, force)
        .await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("models attempted: {}", summary.attempted);
        println!("models cached: {}", summary.cached);
        println!("web search supported: {}", summary.supported);
        println!("web search unsupported: {}", summary.unsupported);
        println!("probes failed: {}", summary.failed);
        for capability in summary.results {
            let status = if capability.error.is_some() {
                "probe-failed"
            } else if capability.supported {
                "supported"
            } else {
                "unsupported"
            };
            println!("{}: {} ({})", capability.model, status, capability.evidence);
        }
    }
    Ok(())
}

pub(super) async fn quota(json_output: bool, provider_filter: Option<&str>) -> anyhow::Result<()> {
    let stored = load_stored_config()?
        .ok_or_else(|| anyhow::anyhow!("provider configuration is missing"))?;
    let registry = ProviderRegistry::new(stored.providers)?;
    if let Some(provider_id) = provider_filter
        && registry.provider(provider_id).is_none()
    {
        anyhow::bail!("unknown provider: {provider_id}");
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut results = Vec::new();
    for provider in registry.providers() {
        if provider_filter.is_some_and(|filter| filter != provider.id()) {
            continue;
        }
        let Some(url) = provider.quota_url() else {
            results.push(serde_json::json!({
                "provider_id": provider.id(),
                "display_name": provider.display_name(),
                "currency": provider.quota_currency(),
                "value": null,
                "error": "quota endpoint is not configured",
                "stale_at": null,
            }));
            continue;
        };
        let result = async {
            let response = provider.apply_auth(client.get(url)).send().await?;
            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                anyhow::bail!("quota endpoint returned {status}: {body}");
            }
            let value: serde_json::Value = serde_json::from_str(&body)?;
            let usage = quota_usage(provider.quota_parser(), &value)?;
            Ok::<_, anyhow::Error>((usage, value))
        }
        .await;
        match result {
            Ok((usage, raw)) => results.push(serde_json::json!({
                "provider_id": provider.id(),
                "display_name": provider.display_name(),
                "currency": provider.quota_currency(),
                "value": usage.used,
                "used": usage.used,
                "limit": usage.limit,
                "remaining": usage.remaining,
                "error": null,
                "stale_at": null,
                "raw": raw,
            })),
            Err(error) => results.push(serde_json::json!({
                "provider_id": provider.id(),
                "display_name": provider.display_name(),
                "currency": provider.quota_currency(),
                "value": null,
                "error": error.to_string(),
                "stale_at": null,
            })),
        }
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    for result in results {
        let provider_id = result["provider_id"].as_str().unwrap_or("<unknown>");
        if let Some(error) = result["error"].as_str() {
            println!("{provider_id}: error: {error}");
        } else {
            let used = &result["used"];
            let limit = &result["limit"];
            let remaining = &result["remaining"];
            let currency = result["currency"].as_str().unwrap_or("");
            if !limit.is_null() {
                println!("{provider_id}: used {used} / {limit} {currency}, remaining {remaining}");
            } else {
                println!("{provider_id}: used {used} {currency}");
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct QuotaUsageSummary {
    pub(super) used: f64,
    pub(super) limit: Option<f64>,
    pub(super) remaining: Option<f64>,
}

pub(super) fn quota_usage(
    parser: ProviderQuotaParser,
    value: &serde_json::Value,
) -> anyhow::Result<QuotaUsageSummary> {
    let (used_fields, limit_fields, remaining_fields): (&[&str], &[&str], &[&str]) = match parser {
        ProviderQuotaParser::BaiduOneApi => (
            &["used_quota", "used", "usage"],
            &[
                "month_quota_limit",
                "quota_limit",
                "limit",
                "total",
                "quota",
            ],
            &["remaining_quota", "remaining", "available"],
        ),
        ProviderQuotaParser::OpenRouter => (
            &["total_usage", "used", "usage"],
            &["total_credits", "limit", "total", "budget"],
            &["remaining", "remaining_quota", "available"],
        ),
        ProviderQuotaParser::Generic => (
            &[
                "used",
                "used_quota",
                "usage",
                "total_usage",
                "total_used",
                "spent",
                "cost",
                "consumed",
                "actual_cost",
            ],
            &[
                "limit",
                "total",
                "total_credits",
                "total_granted",
                "quota",
                "quota_limit",
                "month_quota_limit",
                "budget",
            ],
            &[
                "remaining",
                "remaining_quota",
                "available",
                "total_available",
                "balance",
            ],
        ),
    };
    let used = first_quota_value(value, used_fields)
        .ok_or_else(|| anyhow::anyhow!("quota response does not contain a valid used amount"))?;
    let reported_remaining = first_quota_value(value, remaining_fields);
    let limit = first_quota_value(value, limit_fields)
        .or_else(|| reported_remaining.map(|remaining| used + remaining));
    let remaining = reported_remaining.or_else(|| limit.map(|limit| (limit - used).max(0.0)));
    Ok(QuotaUsageSummary {
        used,
        limit,
        remaining,
    })
}

fn first_quota_value(value: &serde_json::Value, fields: &[&str]) -> Option<f64> {
    [
        "",
        "/data",
        "/quota",
        "/data/quota",
        "/usage",
        "/data/usage",
        "/usage/total",
        "/data/usage/total",
    ]
    .iter()
    .find_map(|base| {
        fields.iter().find_map(|field| {
            let pointer = if base.is_empty() {
                format!("/{field}")
            } else {
                format!("{base}/{field}")
            };
            value.pointer(&pointer).and_then(json_f64)
        })
    })
    .filter(|value| value.is_finite() && *value >= 0.0)
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[cfg(test)]
pub(super) fn summarize_quota_json(value: &serde_json::Value) -> String {
    for base in [
        "",
        "/data",
        "/quota",
        "/data/quota",
        "/usage",
        "/data/usage",
    ] {
        if let Some(used) = first_json_number(
            value,
            base,
            &[
                "used",
                "used_quota",
                "usage",
                "total_usage",
                "spent",
                "cost",
                "consumed",
            ],
        ) {
            let limit = first_json_number(
                value,
                base,
                &[
                    "limit",
                    "total",
                    "total_credits",
                    "quota",
                    "quota_limit",
                    "month_quota_limit",
                    "budget",
                ],
            );
            let remaining =
                first_json_number(value, base, &["remaining", "remaining_quota", "available"]);
            if let Some(limit) = limit {
                if let Some(remaining) = remaining {
                    return format!("quota used: {used} / {limit}, remaining: {remaining}");
                }
                return format!("quota used: {used} / {limit}");
            }
            return format!("quota used: {used}");
        }
    }
    for path in [
        "/remaining",
        "/quota/remaining",
        "/data/remaining",
        "/data/quota/remaining",
        "/data/available",
        "/data/used",
        "/data/total",
        "/data/ratio",
        "/balance",
        "/data/balance",
        "/data/quota",
        "/total_available",
    ] {
        if let Some(value) = value.pointer(path) {
            return format!("quota {path}: {value}");
        }
    }
    value.to_string()
}

#[cfg(test)]
pub(super) fn first_json_number(
    value: &serde_json::Value,
    base: &str,
    fields: &[&str],
) -> Option<serde_json::Number> {
    fields.iter().find_map(|field| {
        let pointer = if base.is_empty() {
            format!("/{field}")
        } else {
            format!("{base}/{field}")
        };
        value.pointer(&pointer).and_then(json_number)
    })
}

#[cfg(test)]
pub(super) fn json_number(value: &serde_json::Value) -> Option<serde_json::Number> {
    match value {
        serde_json::Value::Number(number) => Some(number.clone()),
        serde_json::Value::String(value) => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64),
        _ => None,
    }
}

pub(super) fn show_config(json_output: bool, scope: ConfigScope) -> anyhow::Result<()> {
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

fn redacted_providers(
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
                "image_generation_path": provider.image_generation_path,
                "quota_url": provider.quota_url,
                "quota_username": provider.quota_username,
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

pub(super) fn print_config_value(
    json_output: bool,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
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

pub(super) fn printable_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => "<unset>".to_owned(),
        other => other.to_string(),
    }
}
