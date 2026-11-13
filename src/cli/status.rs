use std::collections::HashSet;
use std::time::Duration;

/// One probe is short so a dead listener fails quickly. A live process that
/// is busy preparing a large context view can miss a single probe, so the
/// status command retries before reporting the gateway as unavailable.
const HEALTH_PROBE_ATTEMPTS: usize = 3;
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const HEALTH_PROBE_RETRY_DELAY: Duration = Duration::from_millis(400);

use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};
use codex_mixin::provider::{ProviderDefinition, ProviderReadinessStatus};
use codex_mixin::server::AppState;
use console::style;

use super::runtime::*;

mod config;
mod opencode_go;
mod quota;
mod usage;

pub(crate) use config::{export_config, import_config, show_config};
pub(crate) use quota::quota;
pub(crate) use usage::{usage, usage_daily, usage_turns};

pub(super) async fn status(json_output: bool) -> anyhow::Result<()> {
    if load_stored_config()?.is_none_or(|config| config.providers.is_empty())
        && !super::providers::official_provider_is_available()?
    {
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
    let snapshot = gateway_snapshot(&config).await?;
    let GatewaySnapshot {
        daemon: metadata,
        daemon_status,
        running_version,
        bind,
        healthz: url,
        health_error,
        activity,
        process_alive,
    } = snapshot;
    if let Some(error) = health_error {
        return print_gateway_unavailable(
            json_output,
            daemon_status,
            metadata.as_ref(),
            &url,
            &config,
            Some(error),
            process_alive,
        );
    }
    let gateway_version = running_version.as_deref().unwrap_or("unknown");
    let endpoint = format!("http://{}/v1", codex_mixin::config::client_address(bind));
    let readiness = provider_readiness_summary(&config.providers);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "configured": true,
                "daemon": daemon_status,
                "pid": metadata.as_ref().map(|metadata| metadata.pid),
                "log": metadata.as_ref().map(|metadata| metadata.log_file.clone()),
                "gateway": "running",
                "gateway_version": gateway_version,
                "activity": activity,
                "official_ech": codex_mixin::application::ech::status()?,
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
        return Ok(());
    }
    println!("daemon: {}", daemon_status.replace('_', " "));
    if let Some(metadata) = &metadata {
        println!("pid: {}", metadata.pid);
        println!("log: {}", metadata.log_file.display());
    }
    println!("{} {gateway_version}", style("gateway-version:").dim());
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
        println!("{} {issue}", style("provider-issue:").yellow());
    }
    Ok(())
}

/// Typed view of the local gateway process and its health endpoint, shared
/// by `status` rendering and service lifecycle decisions.
pub(super) struct GatewaySnapshot {
    pub(super) activity: serde_json::Value,
    pub(super) daemon: Option<DaemonMetadata>,
    pub(super) daemon_status: &'static str,
    /// Version reported by the live runtime metadata, if any.
    pub(super) running_version: Option<String>,
    pub(super) bind: std::net::SocketAddr,
    pub(super) healthz: String,
    /// `None` when `/healthz` answered with success.
    pub(super) health_error: Option<String>,
    /// The recorded daemon or foreground process still exists. Distinct from
    /// [`Self::healthy`]: a stalled listener keeps this true.
    process_alive: bool,
}

impl GatewaySnapshot {
    pub(super) fn healthy(&self) -> bool {
        self.health_error.is_none()
    }

    pub(super) fn daemon_running(&self) -> bool {
        self.daemon_status == "running"
    }

    pub(super) fn process_alive(&self) -> bool {
        self.process_alive
    }
}

pub(super) async fn gateway_snapshot(config: &GatewayConfig) -> anyhow::Result<GatewaySnapshot> {
    let metadata = load_daemon_metadata()?;
    let runtime = load_runtime_metadata()?;
    let daemon_status = match &metadata {
        Some(metadata) if pid_is_running(metadata.pid)? => "running",
        Some(_) => "stale",
        None => "not_started",
    };
    let live_runtime = match runtime {
        Some(runtime)
            if metadata
                .as_ref()
                .is_some_and(|daemon| daemon.pid == runtime.pid) =>
        {
            (daemon_status == "running").then_some(runtime)
        }
        Some(runtime) if pid_is_running(runtime.pid)? => Some(runtime),
        _ => None,
    };
    let bind = match (&live_runtime, &metadata) {
        (Some(runtime), _) => runtime.bind,
        (None, Some(metadata)) => metadata.bind,
        (None, None) => config.bind,
    };
    let healthz = format!(
        "http://{}/healthz",
        codex_mixin::config::client_address(bind)
    );
    let process_alive = live_runtime.is_some() || daemon_status == "running";
    let (activity, health_error) = probe_gateway_health(&healthz, process_alive).await?;
    Ok(GatewaySnapshot {
        activity,
        daemon: metadata,
        daemon_status,
        running_version: live_runtime.and_then(|runtime| runtime.version),
        bind,
        healthz,
        health_error,
        process_alive,
    })
}

/// Probe `/healthz`, retrying a transport failure. A busy gateway can miss one
/// short timeout without having exited.
async fn probe_gateway_health(
    healthz: &str,
    process_alive: bool,
) -> anyhow::Result<(serde_json::Value, Option<String>)> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(if process_alive {
            HEALTH_PROBE_TIMEOUT
        } else {
            HEALTH_CONNECT_TIMEOUT
        })
        .timeout(HEALTH_PROBE_TIMEOUT)
        .build()?;
    let mut last_error = None;
    for attempt in 0..HEALTH_PROBE_ATTEMPTS {
        match client.get(healthz).send().await {
            Ok(response) if response.status().is_success() => {
                let activity = match response.json::<serde_json::Value>().await {
                    Ok(report) => report["activity"].clone(),
                    Err(error) => {
                        tracing::warn!(%error, "invalid gateway health report");
                        serde_json::Value::Null
                    }
                };
                return Ok((activity, None));
            }
            Ok(response) => {
                return Ok((
                    serde_json::Value::Null,
                    Some(format!(
                        "gateway unhealthy: {healthz} returned {}",
                        response.status()
                    )),
                ));
            }
            Err(error) => {
                // Windows can spend seconds retrying a refused TCP connection.
                // A missing listener with no live gateway needs no retry; keep
                // the response deadline and retries for an existing busy one.
                let retry = process_alive || !error.is_connect();
                last_error = Some(error);
                if !retry {
                    break;
                }
            }
        }
        if attempt + 1 < HEALTH_PROBE_ATTEMPTS {
            tokio::time::sleep(HEALTH_PROBE_RETRY_DELAY).await;
        }
    }
    let error = last_error.expect("a failed probe records its error");
    Ok((
        serde_json::Value::Null,
        Some(format!("gateway not running at {healthz}: {error}")),
    ))
}

fn print_gateway_unavailable(
    json_output: bool,
    daemon_status: &str,
    metadata: Option<&DaemonMetadata>,
    healthz: &str,
    config: &GatewayConfig,
    error: Option<String>,
    process_alive: bool,
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
                "gateway": if process_alive { "unresponsive" } else { "stopped" },
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
        let label = if process_alive {
            style("unresponsive").yellow()
        } else {
            style("stopped").red()
        };
        println!("{} {label}", style("gateway:").dim());
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
