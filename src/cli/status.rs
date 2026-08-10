use std::collections::HashSet;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};
use codex_mixin::provider::{
    ProviderDefinition, ProviderQuotaParser, ProviderReadinessStatus, ProviderRegistry,
};
use codex_mixin::server::AppState;
use console::style;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::ConfigScope;
use super::runtime::*;

pub(super) async fn status(json_output: bool) -> anyhow::Result<()> {
    if load_stored_config()?.is_none() {
        if json_output {
            println!(
                "{}",
                serde_json::json!({
                    "configured": false,
                    "config_path": stored_config_path(),
                    "next_command": "codex-mixin provider add --preset <preset> --key <key>"
                })
            );
        } else {
            println!("Codex Mixin is not configured yet.");
            println!("Next: codex-mixin provider add --preset <preset> --key <key>");
            println!("Then: codex-mixin doctor");
        }
        return Ok(());
    }
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
                    "{} {}",
                    style("gateway-version:").dim(),
                    if runtime_running {
                        runtime
                            .as_ref()
                            .and_then(|metadata| metadata.version.as_deref())
                            .unwrap_or("unknown")
                    } else {
                        "unknown"
                    }
                );
                println!("{} {}", style("gateway:").dim(), style("running").green());
                println!("{} {url}", style("healthz:").dim());
                println!("{} {endpoint}", style("endpoint:").dim());
                let readiness_styled = match readiness.0 {
                    "healthy" => style("healthy").green(),
                    "degraded" => style("degraded").yellow(),
                    _ => style("disabled").red(),
                };
                println!("{} {readiness_styled}", style("provider-readiness:").dim());
                println!(
                    "{} {} total, {} healthy, {} degraded, {} disabled",
                    style("providers:").dim(),
                    config.providers.len(),
                    style(readiness.1).green(),
                    if readiness.2 > 0 {
                        style(readiness.2).yellow()
                    } else {
                        style(readiness.2).dim()
                    },
                    if readiness.3 > 0 {
                        style(readiness.3).red()
                    } else {
                        style(readiness.3).dim()
                    },
                );
                for issue in provider_readiness_issue_descriptions(&config.providers) {
                    println!("{} {issue}", style("⚠").yellow());
                }
            }
            Ok(())
        }
        Ok(response) => print_gateway_unavailable(
            json_output,
            daemon_status,
            metadata.as_ref(),
            &url,
            &config,
            Some(format!(
                "gateway unhealthy: {url} returned {}",
                response.status()
            )),
        ),
        Err(err) => print_gateway_unavailable(
            json_output,
            daemon_status,
            metadata.as_ref(),
            &url,
            &config,
            Some(format!("gateway not running at {url}: {err}")),
        ),
    }
}

fn print_gateway_unavailable(
    json_output: bool,
    daemon_status: &str,
    metadata: Option<&DaemonMetadata>,
    healthz: &str,
    config: &GatewayConfig,
    error: Option<String>,
) -> anyhow::Result<()> {
    let readiness = provider_readiness_summary(&config.providers);
    let total = config.providers.len();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "configured": true,
                "daemon": daemon_status,
                "pid": metadata.map(|metadata| metadata.pid),
                "log": metadata.map(|metadata| metadata.log_file.clone()),
                "gateway": "stopped",
                "healthz": healthz,
                "providers": {
                    "total": total,
                    "healthy": readiness.1,
                    "degraded": readiness.2,
                    "disabled": readiness.3,
                },
                "next_command": "codex-mixin service start",
                "error": error,
            }))?
        );
    } else {
        println!("{} {}", style("gateway:").dim(), style("stopped").red());
        println!("{} {healthz}", style("healthz:").dim());
        println!(
            "{} {} total, {} healthy, {} degraded, {} disabled",
            style("providers:").dim(),
            total,
            readiness.1,
            readiness.2,
            readiness.3
        );
        println!("{} codex-mixin service start", style("→").cyan());
        if let Some(error) = error {
            println!("{} {error}", style("detail:").dim());
        }
    }
    Ok(())
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
            descriptions.push(format!("{provider_name}: API key is not configured"));
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
                descriptions.push(format!("{provider_name}: no enabled models are available"));
            }
        } else {
            descriptions.push(format!(
                "{provider_name}: model(s) currently unreachable: {}",
                unavailable_models.join(", ")
            ));
        }
        if let Some(error) = provider.models_refresh_error.as_deref() {
            descriptions.push(format!(
                "{provider_name}: model list refresh failed: {}",
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
                "Baidu OneAPI: model(s) currently unreachable: unreachable-model",
                "Baidu OneAPI: model list refresh failed: request failed upstream returned 503",
            ]
        );
    }
}

#[cfg(test)]
mod opencode_go_quota_tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, header};
    use axum::response::Html;
    use axum::routing::get;
    use codex_mixin::provider::open_code_go_provider;

    use super::*;

    #[test]
    fn parses_opencode_go_dashboard_usage_in_ssr_and_data_slot_formats() {
        let ssr = concat!(
            "<script>",
            "rollingUsage:$R[10]={usagePercent:7.5,resetInSec:18000}",
            "weeklyUsage:$R[11]={resetInSec:540000,usagePercent:2.25}",
            "monthlyUsage:$R[12]={usagePercent:16.75,resetInSec:2480000}",
            "</script>",
        );
        let windows = parse_opencode_go_usage_html(ssr).unwrap();
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].used_percent, 7.5);
        assert_eq!(windows[0].reset_in_sec, 18_000.0);
        assert_eq!(windows[1].used_percent, 2.25);
        assert_eq!(windows[2].used_percent, 16.75);

        let data_slot = r#"
            <div data-slot="usage">
              <div data-slot="usage-item">
                <span data-slot="usage-label">Weekly Usage</span>
                <span data-slot="usage-value"><!--$-->42.5<!--/-->%</span>
                <span data-slot="reset-time"><!--$-->Resets in<!--/--> 1 hour 30 minutes</span>
              </div>
            </div>
        "#;
        let windows = parse_opencode_go_usage_html(data_slot).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, 42.5);
        assert_eq!(windows[0].reset_in_sec, 5_400.0);
        assert!(parse_opencode_go_usage_html("<html>empty</html>").is_none());
    }

    #[test]
    fn parses_opencode_go_billing_in_ssr_and_data_slot_formats() {
        let ssr = concat!(
            "$R[1]={billing:{balance:4250000000,monthlyLimit:100,monthlyUsage:575000000}}",
            "$R[\"payment.list\"]=[{\"amount\":2100000000}]",
        );
        let billing = parse_opencode_go_billing_html(ssr).unwrap();
        assert_eq!(billing.balance_usd, 42.5);
        assert_eq!(billing.monthly_limit_usd, Some(100.0));
        assert_eq!(billing.monthly_usage_usd, Some(5.75));

        let data_slot = r#"
            <div data-slot="billing-item">
              <span data-slot="billing-label">Balance</span>
              <span data-slot="billing-value">$12.34</span>
            </div>
            <div data-slot="billing-item">
              <span data-slot="billing-label">Monthly Limit</span>
              <span data-slot="billing-value">$100.00</span>
            </div>
        "#;
        let billing = parse_opencode_go_billing_html(data_slot).unwrap();
        assert_eq!(billing.balance_usd, 12.34);
        assert_eq!(billing.monthly_limit_usd, Some(100.0));
        assert!(billing.monthly_usage_usd.is_none());
    }

    #[test]
    fn redacted_provider_config_never_returns_the_opencode_go_auth_cookie() {
        let mut provider = open_code_go_provider("opencode-go", "api-key");
        provider.quota_workspace_id = Some("wrk_abc".to_owned());
        provider.quota_auth_cookie = Some("cookie-secret".to_owned());

        let serialized = serde_json::to_string(&redacted_providers(&[provider])).unwrap();

        assert!(serialized.contains("\"quota_workspace_id\":\"wrk_abc\""));
        assert!(serialized.contains("\"quota_auth_cookie\":\"<redacted>\""));
        assert!(!serialized.contains("cookie-secret"));
    }

    #[tokio::test]
    async fn fetches_opencode_go_usage_and_balance_with_auth_cookie_only() {
        let requests = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
        let captured_requests = Arc::clone(&requests);
        let app = Router::new()
            .route(
                "/workspace/wrk_abc/go",
                get(
                    |State(requests): State<Arc<Mutex<Vec<HeaderMap>>>>,
                     headers: HeaderMap| async move {
                        requests.lock().unwrap().push(headers);
                        Html(concat!(
                            "rollingUsage:$R[1]={usagePercent:7,resetInSec:18000}",
                            "weeklyUsage:$R[2]={usagePercent:2,resetInSec:540000}",
                            "monthlyUsage:$R[3]={usagePercent:16,resetInSec:2480000}",
                        ))
                    },
                ),
            )
            .route(
                "/workspace/wrk_abc/billing",
                get(
                    |State(requests): State<Arc<Mutex<Vec<HeaderMap>>>>,
                     headers: HeaderMap| async move {
                        requests.lock().unwrap().push(headers);
                        Html("$R[1]={billing:{balance:500000000,monthlyLimit:100}}")
                    },
                ),
            )
            .with_state(requests);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut provider = open_code_go_provider("opencode-go", "api-key");
        provider.quota_workspace_id = Some("wrk_abc".to_owned());
        provider.quota_auth_cookie = Some("cookie-secret".to_owned());

        let results = fetch_opencode_go_quota_results(
            &reqwest::Client::new(),
            &provider,
            &format!("http://{address}"),
        )
        .await;

        assert_eq!(results.len(), 4);
        assert_eq!(results[0]["display_name"], "OpenCode Go 5h");
        assert_eq!(results[0]["used"], 7.0);
        assert_eq!(results[0]["remaining"], 93.0);
        assert!(results[0]["reset_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(results[1]["display_name"], "OpenCode Go Weekly");
        assert_eq!(results[2]["display_name"], "OpenCode Go Monthly");
        assert_eq!(results[3]["display_name"], "OpenCode Go Balance");
        assert_eq!(results[3]["remaining"], 5.0);
        assert_eq!(results[3]["currency"], "USD");
        assert!(results.iter().all(|result| result["error"].is_null()));

        let captured = captured_requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        for headers in captured.iter() {
            assert_eq!(
                headers.get(header::COOKIE).unwrap().to_str().unwrap(),
                "auth=cookie-secret"
            );
            assert!(headers.get(header::AUTHORIZATION).is_none());
        }
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
        if provider.quota_parser() == ProviderQuotaParser::OpenCodeGo {
            results.extend(
                fetch_opencode_go_quota_results(
                    &client,
                    provider.definition(),
                    OPENCODE_GO_DASHBOARD_BASE,
                )
                .await,
            );
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
                "currency": usage.currency.as_deref().or(provider.quota_currency()),
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
        let provider_label = result["display_name"]
            .as_str()
            .unwrap_or_else(|| result["provider_id"].as_str().unwrap_or("<unknown>"));
        if let Some(error) = result["error"].as_str() {
            println!("{provider_label}: error: {error}");
        } else {
            let used = &result["used"];
            let limit = &result["limit"];
            let remaining = &result["remaining"];
            let currency = result["currency"].as_str().unwrap_or("");
            if !used.is_null() && !limit.is_null() {
                println!(
                    "{provider_label}: used {used} / {limit} {currency}, remaining {remaining}"
                );
            } else if !used.is_null() {
                println!("{provider_label}: used {used} {currency}");
            } else {
                println!("{provider_label}: remaining {remaining} {currency}");
            }
        }
    }
    Ok(())
}

pub(super) async fn usage(json_output: bool) -> anyhow::Result<()> {
    let runtime =
        load_runtime_metadata()?.ok_or_else(|| anyhow::anyhow!("gateway is not running"))?;
    if !pid_is_running(runtime.pid)? {
        anyhow::bail!("gateway is not running");
    }
    let config = GatewayConfig::from_stored_config()?;
    let url = format!("http://{}/v1/usage", runtime.bind);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut request = client.get(&url);
    if let Some(key) = config.gateway_api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("usage gateway request failed ({status}): {body}");
    }
    let rows: Vec<ProviderTokenUsageRow> = serde_json::from_str(&body)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no provider token usage recorded");
        return Ok(());
    }
    for row in rows {
        let cache_hit = row
            .cache_hit_percent
            .map(|percent| format!(", cache hit {percent:.1}%"))
            .unwrap_or_default();
        println!(
            "{}/{}: {} requests, {} uncached input tokens, {} cached tokens, {} cache creation tokens, {} output tokens{cache_hit}",
            row.provider_id,
            row.model_id,
            row.request_count,
            row.input_tokens,
            row.cache_read_tokens,
            row.cache_creation_tokens,
            row.output_tokens
        );
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
struct ProviderTokenUsageRow {
    provider_id: String,
    model_id: String,
    request_count: u64,
    input_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    output_tokens: u64,
    cache_hit_percent: Option<f64>,
}

const OPENCODE_GO_DASHBOARD_BASE: &str = "https://opencode.ai";
const OPENCODE_GO_SCRAPE_TIMEOUT: Duration = Duration::from_secs(10);
const OPENCODE_GO_UNITS_PER_USD: f64 = 100_000_000.0;
const OPENCODE_GO_PATH_ENCODE: &AsciiSet =
    &CONTROLS.add(b' ').add(b'/').add(b'?').add(b'#').add(b'%');

#[derive(Clone, Debug)]
struct OpenCodeGoWindowUsage {
    used_percent: f64,
    reset_in_sec: f64,
}

#[derive(Clone, Debug)]
struct OpenCodeGoBilling {
    balance_usd: f64,
    monthly_limit_usd: Option<f64>,
    monthly_usage_usd: Option<f64>,
}

async fn fetch_opencode_go_quota_results(
    client: &reqwest::Client,
    provider: &ProviderDefinition,
    dashboard_base: &str,
) -> Vec<serde_json::Value> {
    let Some(workspace_id) = provider
        .quota_workspace_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return vec![opencode_go_error_result(
            provider,
            provider.display_name.clone(),
            "quota endpoint is not configured".to_owned(),
        )];
    };
    let Some(auth_cookie) = provider
        .quota_auth_cookie
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return vec![opencode_go_error_result(
            provider,
            provider.display_name.clone(),
            "quota endpoint is not configured".to_owned(),
        )];
    };
    let encoded_workspace_id =
        utf8_percent_encode(workspace_id, OPENCODE_GO_PATH_ENCODE).to_string();
    let usage_url = format!("{dashboard_base}/workspace/{encoded_workspace_id}/go");
    let billing_url = format!("{dashboard_base}/workspace/{encoded_workspace_id}/billing");
    let (usage_result, billing_result) = tokio::join!(
        fetch_opencode_go_usage(client, &usage_url, auth_cookie),
        fetch_opencode_go_billing(client, &billing_url, auth_cookie),
    );
    let mut results = Vec::new();
    match usage_result {
        Ok(windows) => {
            for (label, window) in [
                ("OpenCode Go 5h", 0),
                ("OpenCode Go Weekly", 1),
                ("OpenCode Go Monthly", 2),
            ]
            .into_iter()
            .filter_map(|(label, index)| windows.get(index).map(|window| (label, window)))
            {
                let used = window.used_percent.clamp(0.0, 100.0);
                results.push(serde_json::json!({
                    "provider_id": provider.id,
                    "display_name": label,
                    "currency": null,
                    "value": used,
                    "used": used,
                    "limit": 100.0,
                    "remaining": (100.0 - used).max(0.0),
                    "error": null,
                    "stale_at": null,
                    "reset_at": unix_time_plus_seconds(window.reset_in_sec),
                }));
            }
        }
        Err(error) => results.push(opencode_go_error_result(
            provider,
            "OpenCode Go".to_owned(),
            error,
        )),
    }
    match billing_result {
        Ok(billing) => results.push(serde_json::json!({
            "provider_id": provider.id,
            "display_name": "OpenCode Go Balance",
            "currency": "USD",
            "value": null,
            "used": null,
            "limit": billing.monthly_limit_usd,
            "remaining": billing.balance_usd,
            "monthly_usage": billing.monthly_usage_usd,
            "error": null,
            "stale_at": null,
        })),
        Err(error) => results.push(opencode_go_error_result(
            provider,
            "OpenCode Go Balance".to_owned(),
            error,
        )),
    }
    results
}

fn opencode_go_error_result(
    provider: &ProviderDefinition,
    display_name: String,
    error: String,
) -> serde_json::Value {
    serde_json::json!({
        "provider_id": provider.id,
        "display_name": display_name,
        "currency": provider.quota_currency,
        "value": null,
        "error": error,
        "stale_at": null,
    })
}

async fn fetch_opencode_go_usage(
    client: &reqwest::Client,
    url: &str,
    auth_cookie: &str,
) -> Result<Vec<OpenCodeGoWindowUsage>, String> {
    let html = fetch_opencode_go_html(client, url, auth_cookie).await?;
    parse_opencode_go_usage_html(&html)
        .ok_or_else(|| "could not parse OpenCode Go dashboard usage".to_owned())
}

async fn fetch_opencode_go_billing(
    client: &reqwest::Client,
    url: &str,
    auth_cookie: &str,
) -> Result<OpenCodeGoBilling, String> {
    let html = fetch_opencode_go_html(client, url, auth_cookie).await?;
    parse_opencode_go_billing_html(&html)
        .ok_or_else(|| "could not parse OpenCode Go billing data".to_owned())
}

async fn fetch_opencode_go_html(
    client: &reqwest::Client,
    url: &str,
    auth_cookie: &str,
) -> Result<String, String> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "text/html")
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Gecko/20100101 Firefox/148.0",
        )
        .header(reqwest::header::COOKIE, format!("auth={auth_cookie}"))
        .timeout(OPENCODE_GO_SCRAPE_TIMEOUT)
        .send()
        .await
        .map_err(|error| redact_opencode_go_message(&error.to_string(), auth_cookie))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("OpenCode Go dashboard error {status}"));
    }
    response
        .text()
        .await
        .map_err(|error| redact_opencode_go_message(&error.to_string(), auth_cookie))
}

fn redact_opencode_go_message(message: &str, auth_cookie: &str) -> String {
    let mut sanitized = message.replace(auth_cookie, "<redacted>").replace(
        |character: char| character == '\n' || character == '\r',
        " ",
    );
    sanitized = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.len() > 240 {
        sanitized.truncate(240);
    }
    sanitized
}

fn parse_opencode_go_usage_html(html: &str) -> Option<Vec<OpenCodeGoWindowUsage>> {
    let ssr = [
        ("rolling", "rollingUsage"),
        ("weekly", "weeklyUsage"),
        ("monthly", "monthlyUsage"),
    ]
    .into_iter()
    .map(|(_, field)| parse_opencode_go_ssr_window(html, field))
    .collect::<Vec<_>>();
    if ssr.iter().any(Option::is_some) {
        return Some(
            ssr.into_iter()
                .flatten()
                .map(|(used_percent, reset_in_sec)| OpenCodeGoWindowUsage {
                    used_percent,
                    reset_in_sec,
                })
                .collect(),
        );
    }
    let data_slot = parse_opencode_go_data_slot_windows(html);
    if data_slot.is_empty() {
        return None;
    }
    let mut windows = Vec::new();
    for label in ["rolling", "weekly", "monthly"] {
        if let Some(window) = data_slot.get(label) {
            windows.push(OpenCodeGoWindowUsage {
                used_percent: window.0,
                reset_in_sec: window.1,
            });
        }
    }
    (!windows.is_empty()).then_some(windows)
}

fn parse_opencode_go_ssr_window(html: &str, field: &str) -> Option<(f64, f64)> {
    let number = r"(-?\d+(?:\.\d+)?)";
    let usage_first = Regex::new(&format!(
        r"{field}:\$R\[\d+\]=\{{[^}}]*usagePercent:{number}[^}}]*resetInSec:{number}[^}}]*\}}"
    ))
    .ok()?;
    if let Some(captures) = usage_first.captures(html) {
        let usage = captures.get(1)?.as_str().parse::<f64>().ok()?;
        let reset = captures.get(2)?.as_str().parse::<f64>().ok()?;
        if usage.is_finite() && reset.is_finite() {
            return Some((usage, reset));
        }
    }
    let reset_first = Regex::new(&format!(
        r"{field}:\$R\[\d+\]=\{{[^}}]*resetInSec:{number}[^}}]*usagePercent:{number}[^}}]*\}}"
    ))
    .ok()?;
    let captures = reset_first.captures(html)?;
    let reset = captures.get(1)?.as_str().parse::<f64>().ok()?;
    let usage = captures.get(2)?.as_str().parse::<f64>().ok()?;
    (usage.is_finite() && reset.is_finite()).then_some((usage, reset))
}

fn parse_opencode_go_data_slot_windows(
    html: &str,
) -> std::collections::HashMap<String, (f64, f64)> {
    let mut windows = std::collections::HashMap::new();
    for item in html.split("data-slot=\"usage-item\"") {
        let label = item
            .split("data-slot=\"usage-label\">")
            .nth(1)
            .and_then(|value| value.split('<').next())
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let usage = match item
            .split("data-slot=\"usage-value\">")
            .nth(1)
            .and_then(|value| Regex::new(r"\d+(?:\.\d+)?").ok()?.find(value))
            .and_then(|matched| matched.as_str().parse::<f64>().ok())
            .filter(|value| value.is_finite())
        {
            Some(value) => value,
            None => continue,
        };
        let reset = match item
            .split("data-slot=\"reset-now\">")
            .nth(1)
            .map(|_| 0.0)
            .or_else(|| {
                item.split("data-slot=\"reset-time\">")
                    .nth(1)
                    .and_then(|value| value.split("</span>").next())
                    .and_then(parse_opencode_go_reset_time)
            }) {
            Some(value) => value,
            None => continue,
        };
        let key = if label.contains("rolling") {
            "rolling"
        } else if label.contains("weekly") {
            "weekly"
        } else if label.contains("monthly") {
            "monthly"
        } else {
            continue;
        };
        windows.insert(key.to_owned(), (usage, reset));
    }
    windows
}

fn parse_opencode_go_reset_time(value: &str) -> Option<f64> {
    let normalized = value
        .to_ascii_lowercase()
        .replace("resets in", "")
        .replace("reset in", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.contains("now") {
        return Some(0.0);
    }
    let mut total = 0.0;
    let mut found = false;
    for (multiplier, suffix) in [
        (86400.0, r"days?"),
        (3600.0, r"hours?"),
        (60.0, r"minutes?"),
        (1.0, r"seconds?"),
    ] {
        if let Some(captures) = Regex::new(&format!(r"(\d+(?:\.\d+)?)\s*{suffix}"))
            .ok()?
            .captures(&normalized)
        {
            let value = captures.get(1)?.as_str().parse::<f64>().ok()?;
            total += value * multiplier;
            found = true;
        }
    }
    found.then_some(total)
}

fn parse_opencode_go_billing_html(html: &str) -> Option<OpenCodeGoBilling> {
    let mut fields = std::collections::HashMap::new();
    let field_re =
        Regex::new(r"\b(balance|monthlyLimit|monthlyUsage)\s*:\s*(\d+(?:\.\d+)?)\b").ok()?;
    for captures in field_re.captures_iter(html) {
        fields.insert(
            captures.get(1)?.as_str().to_owned(),
            captures.get(2)?.as_str().parse::<f64>().ok()?,
        );
    }
    if let Some(balance_units) = fields.get("balance").copied() {
        let balance_usd = balance_units / OPENCODE_GO_UNITS_PER_USD;
        let monthly_limit_usd = fields.get("monthlyLimit").copied();
        let monthly_usage_usd = fields
            .get("monthlyUsage")
            .copied()
            .map(|units| units / OPENCODE_GO_UNITS_PER_USD);
        if balance_usd.is_finite() && balance_usd >= 0.0 {
            return Some(OpenCodeGoBilling {
                balance_usd,
                monthly_limit_usd,
                monthly_usage_usd,
            });
        }
    }
    parse_opencode_go_data_slot_billing(html)
}

fn parse_opencode_go_data_slot_billing(html: &str) -> Option<OpenCodeGoBilling> {
    let mut balance_usd = None;
    let mut monthly_limit_usd = None;
    let mut monthly_usage_usd = None;
    for item in html.split("data-slot=\"billing-item\"") {
        let label = item
            .split("data-slot=\"billing-label\">")
            .nth(1)
            .and_then(|value| value.split('<').next())
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let Some(value) = item
            .split("data-slot=\"billing-value\">")
            .nth(1)
            .and_then(|value| {
                Regex::new(r"\$?\s*(\d+(?:,\d{3})*(?:\.\d+)?)")
                    .ok()?
                    .captures(value)
            })
            .and_then(|captures| captures.get(1))
            .and_then(|matched| matched.as_str().replace(',', "").parse::<f64>().ok())
        else {
            continue;
        };
        if !value.is_finite() || value < 0.0 {
            continue;
        }
        if label.contains("balance") {
            balance_usd = Some(value);
        } else if label.contains("monthly") && label.contains("limit") {
            monthly_limit_usd = Some(value);
        } else if label.contains("monthly") && label.contains("usage") {
            monthly_usage_usd = Some(value);
        }
    }
    Some(OpenCodeGoBilling {
        balance_usd: balance_usd?,
        monthly_limit_usd,
        monthly_usage_usd,
    })
}

fn unix_time_plus_seconds(seconds: f64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    unix_seconds_to_rfc3339(now + seconds.max(0.0) as u64)
}

fn unix_seconds_to_rfc3339(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: u64) -> (u64, u64, u64) {
    let z = days_since_epoch as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year as u64, month as u64, day as u64)
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct QuotaUsageSummary {
    pub(super) used: Option<f64>,
    pub(super) limit: Option<f64>,
    pub(super) remaining: Option<f64>,
    pub(super) currency: Option<String>,
}

pub(super) fn quota_usage(
    parser: ProviderQuotaParser,
    value: &serde_json::Value,
) -> anyhow::Result<QuotaUsageSummary> {
    if parser == ProviderQuotaParser::DeepSeek {
        return deepseek_quota_usage(value);
    }
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
        ProviderQuotaParser::DeepSeek => unreachable!("DeepSeek quota handled above"),
        ProviderQuotaParser::OpenCodeGo => {
            unreachable!("OpenCode Go quota is fetched from the dashboard HTML")
        }
    };
    let used = first_quota_value(value, used_fields)
        .ok_or_else(|| anyhow::anyhow!("quota response does not contain a valid used amount"))?;
    let reported_remaining = first_quota_value(value, remaining_fields);
    let limit = first_quota_value(value, limit_fields)
        .or_else(|| reported_remaining.map(|remaining| used + remaining));
    let remaining = reported_remaining.or_else(|| limit.map(|limit| (limit - used).max(0.0)));
    Ok(QuotaUsageSummary {
        used: Some(used),
        limit,
        remaining,
        currency: None,
    })
}

fn deepseek_quota_usage(value: &serde_json::Value) -> anyhow::Result<QuotaUsageSummary> {
    let balances = value
        .get("balance_infos")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("DeepSeek quota response has no balance_infos array"))?;
    let parsed = balances
        .iter()
        .filter_map(|entry| {
            let amount = entry.get("total_balance").and_then(json_f64)?;
            (amount.is_finite() && amount >= 0.0).then(|| {
                let currency = entry
                    .get("currency")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|currency| {
                        currency.len() == 3
                            && currency.bytes().all(|byte| byte.is_ascii_alphabetic())
                    })
                    .map(str::to_ascii_uppercase);
                (amount, currency)
            })
        })
        .collect::<Vec<_>>();
    let balance = parsed
        .iter()
        .find(|(amount, _)| *amount > 0.0)
        .or_else(|| parsed.first())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("DeepSeek quota response has no valid total_balance"))?;
    Ok(QuotaUsageSummary {
        used: None,
        limit: None,
        remaining: Some(balance.0),
        currency: balance.1,
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
