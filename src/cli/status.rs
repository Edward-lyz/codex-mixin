use std::collections::HashSet;
use std::time::Duration;

use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};
use codex_mixin::provider::{ProviderDefinition, ProviderReadinessStatus};
use codex_mixin::server::AppState;
use console::style;

use super::runtime::*;

mod config;
mod opencode_go;
mod quota;
mod usage;

#[cfg(test)]
pub(crate) use config::redacted_providers;
pub(crate) use config::{export_config, show_config};
#[cfg(test)]
pub(crate) use opencode_go::{
    fetch_opencode_go_quota_results, official_quota_rows, parse_opencode_go_billing_html,
    parse_opencode_go_usage_html,
};
pub(crate) use quota::quota;
#[cfg(test)]
pub(crate) use quota::summarize_quota_json;
pub(crate) use usage::usage;

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
        if !provider.auth.is_configured() {
            descriptions.push(format!("{provider_name}: credentials are not configured"));
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
    use codex_mixin::provider::ProviderRegistry;
    use codex_mixin::provider::open_code_go_provider;

    use super::*;

    #[test]
    fn maps_official_primary_and_secondary_rate_limit_windows() {
        let rows = official_quota_rows(&serde_json::json!({
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "limitName": "Codex",
                    "primary": {
                        "usedPercent": 25,
                        "windowDurationMins": 300,
                        "resetsAt": 1_730_947_200_u64
                    },
                    "secondary": {
                        "usedPercent": 40,
                        "windowDurationMins": 10_080,
                        "resetsAt": 1_731_551_999_u64
                    }
                }
            }
        }))
        .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["provider_id"], "official");
        assert_eq!(rows[0]["quota_id"], "codex.primary");
        assert_eq!(rows[0]["label"], "Codex · 5h");
        assert_eq!(rows[0]["used"], 25.0);
        assert_eq!(rows[0]["remaining"], 75.0);
        assert_eq!(rows[1]["quota_id"], "codex.secondary");
        assert_eq!(rows[1]["label"], "Codex · 7d");
        assert!(rows[1]["reset_at"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn rejects_explicit_quota_queries_for_disabled_providers() {
        let mut provider = codex_mixin::provider::custom_provider("disabled", "secret");
        provider.base_url = "https://example.com".to_owned();
        provider.enabled = false;
        let registry = ProviderRegistry::new(vec![provider]).unwrap();

        let error =
            super::quota::validate_quota_filter(&registry, false, Some("disabled")).unwrap_err();

        assert_eq!(error.to_string(), "provider is disabled: disabled");
    }

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
        assert_eq!(windows[0].quota_id, "five_hour");
        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].used_percent, 7.5);
        assert_eq!(windows[0].reset_in_sec, 18_000.0);
        assert_eq!(windows[1].quota_id, "weekly");
        assert_eq!(windows[1].used_percent, 2.25);
        assert_eq!(windows[2].quota_id, "monthly");
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
        assert_eq!(windows[0].quota_id, "weekly");
        assert_eq!(windows[0].label, "Weekly");
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
        assert_eq!(results[0]["provider_display_name"], "OpenCode Go");
        assert_eq!(results[0]["display_name"], "OpenCode Go 5h");
        assert_eq!(results[0]["quota_id"], "five_hour");
        assert_eq!(results[0]["label"], "5h");
        assert_eq!(results[0]["used"], 7.0);
        assert_eq!(results[0]["remaining"], 93.0);
        assert!(results[0]["reset_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(results[1]["display_name"], "OpenCode Go Weekly");
        assert_eq!(results[1]["quota_id"], "weekly");
        assert_eq!(results[2]["display_name"], "OpenCode Go Monthly");
        assert_eq!(results[2]["quota_id"], "monthly");
        assert_eq!(results[3]["display_name"], "OpenCode Go Balance");
        assert_eq!(results[3]["quota_id"], "balance");
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
