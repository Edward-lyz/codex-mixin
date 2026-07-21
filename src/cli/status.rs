use std::time::{Duration, Instant};

use codex_mixin::config::{
    GatewayConfig, ProviderPreset, StoredGatewayConfig, load_stored_config, stored_config_path,
};
use codex_mixin::server::AppState;

use super::ConfigScope;
use super::config_input::first_env_value;
use super::runtime::*;

pub(super) async fn doctor() -> anyhow::Result<()> {
    let path = stored_config_path();
    println!("config: {}", path.display());
    match load_stored_config()? {
        Some(config) => {
            println!(
                "stored upstream: {}",
                config.upstream_base_url.as_deref().unwrap_or("<missing>")
            );
            println!(
                "stored key: {}",
                if config.upstream_api_key.is_some() {
                    "configured"
                } else {
                    "missing"
                }
            );
            println!(
                "quota: {}",
                config.quota_url.as_deref().unwrap_or("not configured")
            );
            println!(
                "quota username: {}",
                config.quota_username.as_deref().unwrap_or("not configured")
            );
        }
        None => println!("stored login: missing"),
    }
    let config = GatewayConfig::from_env()?;
    println!("bind: {}", config.bind);
    println!("upstream: {}", config.upstream_base_url);
    println!(
        "gateway auth: {}",
        if config.gateway_api_key.is_some() {
            "configured"
        } else {
            "disabled"
        }
    );
    let state = AppState::new(config)?;
    let started = Instant::now();
    let models = state.fetch_models().await?;
    println!(
        "models endpoint: ok, {} models, {} ms",
        models.len(),
        started.elapsed().as_millis()
    );
    println!("doctor: ok");
    Ok(())
}

pub(super) async fn status() -> anyhow::Result<()> {
    let config = GatewayConfig::from_env()?;
    let metadata = load_daemon_metadata()?;
    let runtime = load_runtime_metadata()?;
    if let Some(metadata) = &metadata {
        println!(
            "daemon: {}",
            if pid_is_running(metadata.pid)? {
                "running"
            } else {
                "stale"
            }
        );
        println!("pid: {}", metadata.pid);
        println!("log: {}", metadata.log_file.display());
    } else {
        println!("daemon: not started");
    }
    let runtime_running = runtime
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
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
            println!("gateway: running");
            println!("healthz: {url}");
            println!("endpoint: http://{bind}/v1");
            Ok(())
        }
        Ok(response) => anyhow::bail!("gateway unhealthy: {} returned {}", url, response.status()),
        Err(err) => anyhow::bail!("gateway not running at {url}: {err}"),
    }
}

pub(super) async fn models(json_output: bool) -> anyhow::Result<()> {
    let config = GatewayConfig::from_env()?;
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
    let config = GatewayConfig::from_env()?;
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

pub(super) async fn quota(json_output: bool) -> anyhow::Result<()> {
    let stored = load_stored_config()?.unwrap_or_default();
    let quota_url = resolve_quota_url(&stored)?;
    let api_key = std::env::var("CODEX_GATEWAY_QUOTA_KEY")
        .or_else(|_| std::env::var("ANTHROPIC_AUTH_TOKEN"))
        .or_else(|_| std::env::var("CODEX_GATEWAY_UPSTREAM_API_KEY"))
        .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
        .ok()
        .filter(|key| !key.is_empty())
        .or(stored.upstream_api_key)
        .ok_or_else(|| anyhow::anyhow!("quota auth key is not configured"))?;
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?
        .get(quota_url)
        .bearer_auth(api_key)
        .send()
        .await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("quota endpoint returned {status}: {body}");
    }
    if json_output {
        match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(value) => println!("{}", serde_json::to_string_pretty(&value)?),
            Err(_) => println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "content_type": content_type,
                    "body": body
                }))?
            ),
        }
        return Ok(());
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
        println!("{}", summarize_quota_json(&value));
    } else {
        println!("{}", body.lines().next().unwrap_or("").trim());
    }
    Ok(())
}

pub(super) fn resolve_quota_url(stored: &StoredGatewayConfig) -> anyhow::Result<reqwest::Url> {
    let provider_preset = first_env_value(&["CODEX_GATEWAY_PROVIDER"])
        .or_else(|| stored.provider_preset.clone())
        .map(|value| ProviderPreset::parse(&value))
        .transpose()?
        .unwrap_or(ProviderPreset::Custom);
    let upstream_base_url =
        first_env_value(&["CODEX_GATEWAY_UPSTREAM_BASE_URL", "ANTHROPIC_BASE_URL"])
            .or_else(|| stored.upstream_base_url.clone())
            .or_else(|| {
                provider_preset
                    .default_base_url()
                    .map(std::borrow::ToOwned::to_owned)
            });
    let quota_url = match std::env::var("CODEX_GATEWAY_QUOTA_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .or_else(|| stored.quota_url.clone())
        .or_else(|| {
            upstream_base_url
                .as_deref()
                .and_then(|base_url| provider_preset.default_quota_url(base_url))
        }) {
        Some(quota_url) => quota_url,
        None => anyhow::bail!(
            "quota URL is not configured for this provider. Set CODEX_GATEWAY_QUOTA_URL or run login --quota-url <url>"
        ),
    };
    let mut url = reqwest::Url::parse(&quota_url)?;
    if !url.query_pairs().any(|(key, _)| key == "username")
        && let Some(username) = quota_username(stored)
    {
        url.query_pairs_mut().append_pair("username", &username);
    }
    Ok(url)
}

pub(super) fn quota_username(stored: &StoredGatewayConfig) -> Option<String> {
    std::env::var("CODEX_GATEWAY_QUOTA_USERNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| stored.quota_username.clone())
}

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
            let value = serde_json::json!({
                "path": path,
                "gateway_bind": stored.gateway_bind,
                "provider_preset": stored.provider_preset,
                "upstream_kind": stored.upstream_kind,
                "upstream_base_url": stored.upstream_base_url,
                "upstream_messages_path": stored.upstream_messages_path,
                "upstream_models_path": stored.upstream_models_path,
                "upstream_image_generation_path": stored.upstream_image_generation_path,
                "upstream_api_key": stored.upstream_api_key.as_ref().map(|_| "<redacted>"),
                "gateway_api_key": stored.gateway_api_key.as_ref().map(|_| "<redacted>"),
                "quota_url": stored.quota_url,
                "quota_username": stored.quota_username,
                "fusion_profiles": stored.fusion_profiles
            });
            print_config_value(json_output, &value)
        }
        ConfigScope::Effective => {
            let config = GatewayConfig::from_env()?;
            let bind = match load_runtime_metadata()? {
                Some(runtime) if pid_is_running(runtime.pid)? => runtime.bind,
                _ => config.bind,
            };
            let value = serde_json::json!({
                "path": path,
                "bind": bind.to_string(),
                "provider_preset": config.provider_preset.as_str(),
                "upstream_kind": config.upstream_kind.as_str(),
                "upstream_base_url": config.upstream_base_url,
                "upstream_messages_path": config.upstream_messages_path,
                "upstream_models_path": config.upstream_models_path,
                "upstream_image_generation_path": config.upstream_image_generation_path,
                "official_image_generation_url": config.official_image_generation_url()?,
                "official_image_edit_url": config.official_image_edit_url()?,
                "official_responses_url": config.official_responses_url,
                "codex_auth_path": config.codex_auth_path,
                "upstream_api_key": "<redacted>",
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
