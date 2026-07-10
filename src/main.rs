use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, value};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use codex_mixin::catalog::{
    codex_catalog_from_models_with_metadata, codex_oauth_proxy_catalog_from_models_with_metadata,
    load_template_catalog, refresh_managed_oauth_catalog,
};
use codex_mixin::config::{
    GatewayConfig, ProviderPreset, StoredGatewayConfig, delete_stored_config, load_stored_config,
    save_stored_config, stored_config_path,
};
use codex_mixin::history::migrate_history_to_custom_provider;
use codex_mixin::model_metadata::{ModelMetadataResolver, default_cache_path};
use codex_mixin::server::{AppState, serve};

const MANAGED_CONFIG_MARKER: &str = "codex-mixin managed config";
const MANAGED_CONFIG_HEADER: &str = "# codex-mixin managed config. Run `codex-mixin uninstall-codex` to restore the previous config.";
const LITELLM_MODEL_METADATA_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(visible_alias = "auth")]
    Login {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        gateway_key: Option<String>,
        #[arg(long)]
        quota_url: Option<String>,
        #[arg(long)]
        quota_username: Option<String>,
    },
    Logout,
    #[command(visible_alias = "check")]
    Doctor,
    Status,
    Models {
        #[arg(long)]
        json: bool,
    },
    Quota {
        #[arg(long)]
        json: bool,
    },
    Config {
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum, default_value_t = ConfigScope::Effective)]
        scope: ConfigScope,
    },
    Start {
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[arg(long)]
        daemon: bool,
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    Stop {
        #[arg(long)]
        force: bool,
    },
    Restart {
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    Logs {
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: usize,
        #[arg(short, long)]
        follow: bool,
    },
    Serve {
        #[arg(long)]
        bind: Option<SocketAddr>,
    },
    Catalog {
        #[arg(long)]
        template_catalog: Option<PathBuf>,
    },
    #[command(name = "refresh-metadata")]
    RefreshMetadata {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    #[command(name = "migrate-history")]
    MigrateHistory {
        #[arg(long)]
        codex_home: Option<PathBuf>,
    },
    #[command(name = "install-codex", visible_alias = "codex-config")]
    InstallCodex {
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        set_default: bool,
        #[arg(long)]
        codex_oauth_proxy: bool,
        #[arg(long, default_value = "custom")]
        provider: String,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        catalog: Option<PathBuf>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long, default_value = "disabled")]
        web_search: String,
        #[arg(long)]
        env_key: Option<String>,
        #[arg(long)]
        no_env_key: bool,
    },
    #[command(name = "uninstall-codex")]
    UninstallCodex {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        catalog: Option<PathBuf>,
    },
    #[command(name = "refresh-codex-catalog")]
    RefreshCodexCatalog,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ConfigScope {
    Stored,
    Effective,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DaemonMetadata {
    pid: u32,
    bind: SocketAddr,
    log_file: PathBuf,
    started_at: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Start {
        bind: None,
        daemon: false,
        log_file: None,
    }) {
        Command::Login {
            provider,
            key,
            base_url,
            gateway_key,
            quota_url,
            quota_username,
        } => login(
            provider,
            key,
            base_url,
            gateway_key,
            quota_url,
            quota_username,
        ),
        Command::Logout => logout(),
        Command::Doctor => doctor().await,
        Command::Status => status().await,
        Command::Models { json } => models(json).await,
        Command::Quota { json } => quota(json).await,
        Command::Config { json, scope } => show_config(json, scope),
        Command::Start {
            bind,
            daemon,
            log_file,
        } => start(bind, daemon, log_file).await,
        Command::Stop { force } => stop(force),
        Command::Restart { bind, log_file } => restart(bind, log_file).await,
        Command::Logs { lines, follow } => logs(lines, follow),
        Command::Serve { bind } => start(bind, false, None).await,
        Command::Catalog { template_catalog } => {
            let config = GatewayConfig::from_env()?;
            let state = AppState::new(config.clone())?;
            let models = state.fetch_models().await?;
            let template = load_template_catalog(template_catalog.as_deref())?;
            let metadata = load_model_metadata_resolver().await?;
            let catalog = codex_catalog_from_models_with_metadata(
                &models,
                config.default_context_window,
                template.as_ref(),
                &metadata,
            );
            println!("{}", serde_json::to_string_pretty(&catalog)?);
            Ok(())
        }
        Command::RefreshMetadata { output } => refresh_metadata(output).await,
        Command::MigrateHistory { codex_home } => migrate_history(codex_home),
        Command::InstallCodex {
            model,
            set_default,
            codex_oauth_proxy,
            provider,
            config,
            catalog,
            base_url,
            web_search,
            env_key,
            no_env_key,
        } => {
            install_codex(
                model,
                set_default,
                codex_oauth_proxy,
                provider,
                config,
                catalog,
                base_url,
                web_search,
                env_key,
                no_env_key,
            )
            .await
        }
        Command::UninstallCodex { config, catalog } => uninstall_codex(config, catalog),
        Command::RefreshCodexCatalog => refresh_default_managed_codex_catalog(),
    }
}

fn login(
    provider: Option<String>,
    key: Option<String>,
    base_url: Option<String>,
    gateway_key: Option<String>,
    quota_url: Option<String>,
    quota_username: Option<String>,
) -> anyhow::Result<()> {
    let existing = load_stored_config()?.unwrap_or_default();
    let provider_preset = provider
        .or(existing.provider_preset.clone())
        .or_else(|| first_env_value(&["CODEX_GATEWAY_PROVIDER"]))
        .map(|value| ProviderPreset::parse(&value))
        .transpose()?
        .unwrap_or(ProviderPreset::Custom);
    let upstream_api_key = match key {
        Some(key) => trim_required("key", key)?,
        None => existing
            .upstream_api_key
            .clone()
            .map(Ok)
            .unwrap_or_else(|| prompt_required("upstream API key"))?,
    };
    let upstream_base_url = normalize_base_url(
        base_url
            .or(existing.upstream_base_url)
            .or_else(|| first_env_value(&["CODEX_GATEWAY_UPSTREAM_BASE_URL", "ANTHROPIC_BASE_URL"]))
            .or_else(|| {
                provider_preset
                    .default_base_url()
                    .map(std::borrow::ToOwned::to_owned)
            })
            .map(Ok)
            .unwrap_or_else(|| prompt_required("upstream base URL"))?,
    )?;
    let upstream_kind = provider_preset.default_upstream_kind();
    let config = StoredGatewayConfig {
        provider_preset: Some(provider_preset.as_str().to_owned()),
        upstream_kind: Some(upstream_kind.as_str().to_owned()),
        upstream_base_url: Some(upstream_base_url.clone()),
        upstream_messages_path: Some(provider_preset.default_messages_path().to_owned()),
        upstream_models_path: Some(provider_preset.default_models_path().to_owned()),
        upstream_api_key: Some(upstream_api_key),
        gateway_api_key: gateway_key
            .map(|key| trim_required("gateway key", key))
            .transpose()?
            .or(existing.gateway_api_key),
        quota_url: quota_url
            .map(normalize_base_url)
            .transpose()?
            .or(existing.quota_url)
            .or_else(|| provider_preset.default_quota_url(&upstream_base_url)),
        quota_username: quota_username
            .map(|username| trim_required("quota username", username))
            .transpose()?
            .or(existing.quota_username),
    };
    let path = save_stored_config(&config)?;
    println!("login saved: {}", path.display());
    println!("provider: {}", provider_preset.as_str());
    println!("upstream kind: {}", upstream_kind.as_str());
    println!("upstream: {upstream_base_url}");
    if config.gateway_api_key.is_some() {
        println!("gateway auth: configured");
    } else {
        println!("gateway auth: disabled");
    }
    if config.quota_url.is_some() {
        println!("quota: configured");
    } else {
        println!("quota: not configured");
    }
    println!(
        "quota username: {}",
        config.quota_username.as_deref().unwrap_or("not configured")
    );
    Ok(())
}

fn logout() -> anyhow::Result<()> {
    let path = stored_config_path();
    if delete_stored_config()? {
        println!("login removed: {}", path.display());
    } else {
        println!("no stored login: {}", path.display());
    }
    Ok(())
}

async fn doctor() -> anyhow::Result<()> {
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

async fn status() -> anyhow::Result<()> {
    let config = GatewayConfig::from_env()?;
    let metadata = load_daemon_metadata()?;
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
    let bind = metadata
        .as_ref()
        .map_or(config.bind, |metadata| metadata.bind);
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
            Ok(())
        }
        Ok(response) => anyhow::bail!("gateway unhealthy: {} returned {}", url, response.status()),
        Err(err) => anyhow::bail!("gateway not running at {url}: {err}"),
    }
}

async fn models(json_output: bool) -> anyhow::Result<()> {
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

async fn quota(json_output: bool) -> anyhow::Result<()> {
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

fn resolve_quota_url(stored: &StoredGatewayConfig) -> anyhow::Result<reqwest::Url> {
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
    if !url.query_pairs().any(|(key, _)| key == "username") {
        if let Some(username) = quota_username(stored) {
            url.query_pairs_mut().append_pair("username", &username);
        }
    }
    Ok(url)
}

fn quota_username(stored: &StoredGatewayConfig) -> Option<String> {
    std::env::var("CODEX_GATEWAY_QUOTA_USERNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| stored.quota_username.clone())
}

fn summarize_quota_json(value: &serde_json::Value) -> String {
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

fn first_json_number(
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

fn json_number(value: &serde_json::Value) -> Option<serde_json::Number> {
    match value {
        serde_json::Value::Number(number) => Some(number.clone()),
        serde_json::Value::String(value) => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64),
        _ => None,
    }
}

fn show_config(json_output: bool, scope: ConfigScope) -> anyhow::Result<()> {
    let path = stored_config_path();
    match scope {
        ConfigScope::Stored => {
            let stored = load_stored_config()?.unwrap_or_default();
            let value = serde_json::json!({
                "path": path,
                "provider_preset": stored.provider_preset,
                "upstream_kind": stored.upstream_kind,
                "upstream_base_url": stored.upstream_base_url,
                "upstream_messages_path": stored.upstream_messages_path,
                "upstream_models_path": stored.upstream_models_path,
                "upstream_api_key": stored.upstream_api_key.as_ref().map(|_| "<redacted>"),
                "gateway_api_key": stored.gateway_api_key.as_ref().map(|_| "<redacted>"),
                "quota_url": stored.quota_url,
                "quota_username": stored.quota_username
            });
            print_config_value(json_output, &value)
        }
        ConfigScope::Effective => {
            let config = GatewayConfig::from_env()?;
            let value = serde_json::json!({
                "path": path,
                "bind": config.bind.to_string(),
                "provider_preset": config.provider_preset.as_str(),
                "upstream_kind": config.upstream_kind.as_str(),
                "upstream_base_url": config.upstream_base_url,
                "upstream_messages_path": config.upstream_messages_path,
                "upstream_models_path": config.upstream_models_path,
                "official_responses_url": config.official_responses_url,
                "codex_auth_path": config.codex_auth_path,
                "upstream_api_key": "<redacted>",
                "gateway_api_key": config.gateway_api_key.as_ref().map(|_| "<redacted>"),
                "accept_codex_oauth": config.accept_codex_oauth,
                "thinking_mode": format!("{:?}", config.thinking_mode),
                "enable_web_search_tool": config.enable_web_search_tool,
                "web_search_tool_type": config.web_search_tool_type,
                "web_search_max_uses": config.web_search_max_uses,
                "web_search_exclusive": config.web_search_exclusive,
                "web_search_omit_system_instructions": config.web_search_omit_system_instructions,
                "web_search_latest_user_only": config.web_search_latest_user_only
            });
            print_config_value(json_output, &value)
        }
    }
}

fn print_config_value(json_output: bool, value: &serde_json::Value) -> anyhow::Result<()> {
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

fn printable_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => "<unset>".to_owned(),
        other => other.to_string(),
    }
}

fn migrate_history(codex_home: Option<PathBuf>) -> anyhow::Result<()> {
    let codex_home = codex_home.unwrap_or_else(codex_home_path);
    let outcome = migrate_history_to_custom_provider(&codex_home)?;
    println!(
        "history jsonl files changed: {}",
        outcome.jsonl_files_changed
    );
    println!(
        "history jsonl lines changed: {}",
        outcome.jsonl_lines_changed
    );
    println!(
        "history sqlite rows changed: {}",
        outcome.sqlite_rows_changed
    );
    if let Some(backup_root) = outcome.backup_root {
        println!("history backup: {}", backup_root.display());
    } else {
        println!("history backup: <none; no changes>");
    }
    Ok(())
}

async fn refresh_metadata(output: Option<PathBuf>) -> anyhow::Result<()> {
    let output = output.unwrap_or_else(default_cache_path);
    let body = fetch_litellm_metadata().await?;
    let parsed: serde_json::Value = serde_json::from_str(&body)?;
    let resolver = ModelMetadataResolver::from_json(&parsed)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, body)?;
    println!("model metadata written: {}", output.display());
    println!("metadata entries: {}", resolver.len());
    Ok(())
}

async fn load_model_metadata_resolver() -> anyhow::Result<ModelMetadataResolver> {
    if let Ok(path) = std::env::var("CODEX_GATEWAY_MODEL_METADATA")
        && !path.is_empty()
    {
        return ModelMetadataResolver::from_json_file(std::path::Path::new(&path));
    }
    let cache_path = default_cache_path();
    if cache_path.exists() {
        return ModelMetadataResolver::from_json_file(&cache_path);
    }
    match fetch_litellm_metadata().await {
        Ok(body) => {
            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            let resolver = ModelMetadataResolver::from_json(&parsed)?;
            if let Some(parent) = cache_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&cache_path, body)?;
            eprintln!(
                "model metadata cached: {} ({} entries)",
                cache_path.display(),
                resolver.len()
            );
            Ok(resolver)
        }
        Err(err) => {
            eprintln!(
                "warning: failed to fetch LiteLLM model metadata: {err}; using built-in family rules"
            );
            Ok(ModelMetadataResolver::empty())
        }
    }
}

async fn fetch_litellm_metadata() -> anyhow::Result<String> {
    let url = std::env::var("CODEX_GATEWAY_MODEL_METADATA_URL")
        .unwrap_or_else(|_| LITELLM_MODEL_METADATA_URL.to_owned());
    let response = reqwest::Client::new().get(&url).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("metadata endpoint returned {status}: {body}");
    }
    Ok(body)
}

async fn install_codex(
    requested_model: Option<String>,
    set_default: bool,
    codex_oauth_proxy: bool,
    requested_provider: String,
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
    base_url: Option<String>,
    web_search: String,
    env_key: Option<String>,
    no_env_key: bool,
) -> anyhow::Result<()> {
    let gateway_config = GatewayConfig::from_env()?;
    let state = AppState::new(gateway_config.clone())?;
    let models = state.fetch_models().await?;
    if models.is_empty() {
        anyhow::bail!("upstream /v1/models returned no models");
    }
    let config_path = config_path.unwrap_or_else(default_codex_config_path);
    let catalog_path = catalog_path.unwrap_or_else(default_codex_catalog_path);
    if let Some(parent) = catalog_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw_config = prepare_managed_config_install(&config_path)?;
    let template = load_template_catalog(None)?;
    let metadata = load_model_metadata_resolver().await?;
    let catalog = if codex_oauth_proxy {
        codex_oauth_proxy_catalog_from_models_with_metadata(
            &models,
            gateway_config.default_context_window,
            template.as_ref(),
            &metadata,
        )
    } else {
        codex_catalog_from_models_with_metadata(
            &models,
            gateway_config.default_context_window,
            template.as_ref(),
            &metadata,
        )
    };
    write_atomic_if_changed(&catalog_path, &serde_json::to_vec_pretty(&catalog)?)?;

    let mut doc = if raw_config.trim().is_empty() {
        DocumentMut::new()
    } else {
        raw_config.parse::<DocumentMut>()?
    };
    let provider = requested_provider;
    validate_provider_name(&provider)?;

    let should_set_default = set_default || requested_model.is_some();
    let selected_model = if codex_oauth_proxy {
        if should_set_default {
            Some(select_codex_oauth_proxy_model(
                requested_model,
                &models,
                template.as_ref(),
                &doc,
            )?)
        } else {
            None
        }
    } else if should_set_default {
        Some(select_codex_model(requested_model, &models, &doc)?)
    } else {
        None
    };
    let gateway_base_url = normalize_base_url(
        base_url.unwrap_or_else(|| format!("http://{}/v1", gateway_config.bind)),
    )?;
    let env_key = if codex_oauth_proxy || no_env_key {
        None
    } else {
        env_key.or_else(|| {
            gateway_config
                .gateway_api_key
                .as_ref()
                .map(|_| "CODEX_GATEWAY_KEY".to_owned())
        })
    };
    upsert_codex_config(
        &mut doc,
        &provider,
        selected_model.as_deref(),
        &catalog_path,
        &gateway_base_url,
        &web_search,
        env_key.as_deref(),
        codex_oauth_proxy,
    )?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&config_path, format!("{MANAGED_CONFIG_HEADER}\n{}", doc))?;
    println!("codex config updated: {}", config_path.display());
    println!(
        "codex config backup: {}",
        managed_backup_path(&config_path).display()
    );
    println!("model catalog written: {}", catalog_path.display());
    println!("models installed: {}", models.len());
    println!("metadata entries loaded: {}", metadata.len());
    println!("provider: {provider}");
    if codex_oauth_proxy {
        println!("codex oauth proxy: enabled");
    }
    if let Some(selected_model) = selected_model {
        println!("default model: {selected_model}");
    } else {
        println!("default model/provider: unchanged");
    }
    println!("base_url: {gateway_base_url}");
    if let Some(env_key) = env_key
        && !codex_oauth_proxy
    {
        println!("env_key: {env_key}");
    }
    println!("reload required: restart Codex app; for Codex CLI, start a new session");
    Ok(())
}

fn uninstall_codex(
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let config_path = config_path.unwrap_or_else(default_codex_config_path);
    let catalog_path = catalog_path.unwrap_or_else(default_codex_catalog_path);
    let raw_config = if config_path.exists() {
        fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    if !is_managed_config(&raw_config) {
        anyhow::bail!(
            "Codex config is not managed by codex-mixin: {}",
            config_path.display()
        );
    }
    let backup_path = managed_backup_path(&config_path);
    let absent_marker_path = managed_absent_marker_path(&config_path);
    if backup_path.exists() {
        fs::copy(&backup_path, &config_path)?;
        fs::remove_file(&backup_path)?;
        println!("codex config restored: {}", config_path.display());
    } else if absent_marker_path.exists() {
        if config_path.exists() {
            fs::remove_file(&config_path)?;
        }
        fs::remove_file(&absent_marker_path)?;
        println!("codex config removed; no previous config existed");
    } else {
        anyhow::bail!(
            "missing managed backup for {}; expected {}",
            config_path.display(),
            backup_path.display()
        );
    }
    if catalog_path.exists() {
        fs::remove_file(&catalog_path)?;
        println!("model catalog removed: {}", catalog_path.display());
    }
    println!("reload required: restart Codex app; for Codex CLI, start a new session");
    Ok(())
}

fn refresh_default_managed_codex_catalog() -> anyhow::Result<()> {
    let config_path = default_codex_config_path();
    let official_catalog_path = codex_home_path().join("models_cache.json");
    if refresh_managed_codex_catalog(&config_path, &official_catalog_path)? {
        println!("Codex model catalog refreshed");
    } else {
        println!("Codex model catalog already current or not managed by codex-mixin");
    }
    Ok(())
}

fn refresh_managed_codex_catalog(
    config_path: &Path,
    official_catalog_path: &Path,
) -> anyhow::Result<bool> {
    if !config_path.exists() {
        return Ok(false);
    }
    let raw_config = fs::read_to_string(config_path)?;
    if !is_managed_config(&raw_config) {
        return Ok(false);
    }
    let doc = raw_config.parse::<DocumentMut>()?;
    let catalog_path = doc
        .get("model_catalog_json")
        .and_then(Item::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("managed Codex config has no model_catalog_json"))?;
    let official_catalog = serde_json::from_slice(&fs::read(official_catalog_path)?)?;
    let managed_catalog = serde_json::from_slice(&fs::read(&catalog_path)?)?;
    let refreshed = refresh_managed_oauth_catalog(&official_catalog, &managed_catalog)?;
    write_atomic_if_changed(&catalog_path, &serde_json::to_vec_pretty(&refreshed)?)
}

fn write_atomic_if_changed(path: &Path, contents: &[u8]) -> anyhow::Result<bool> {
    if path.exists() && fs::read(path)? == contents {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model-catalog.json");
    let temporary_path = path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    fs::write(&temporary_path, contents)?;
    if let Err(err) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(err.into());
    }
    Ok(true)
}

fn prepare_managed_config_install(config_path: &std::path::Path) -> anyhow::Result<String> {
    let raw_config = if config_path.exists() {
        fs::read_to_string(config_path)?
    } else {
        String::new()
    };
    if is_managed_config(&raw_config) {
        return Ok(raw_config);
    }
    let backup_path = managed_backup_path(config_path);
    let absent_marker_path = managed_absent_marker_path(config_path);
    if backup_path.exists() || absent_marker_path.exists() {
        anyhow::bail!(
            "existing codex-mixin backup found but current config is not managed. Restore or remove {} first",
            backup_path.display()
        );
    }
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if config_path.exists() {
        fs::copy(config_path, &backup_path)?;
    } else {
        fs::write(&absent_marker_path, b"")?;
    }
    Ok(raw_config)
}

fn is_managed_config(raw_config: &str) -> bool {
    raw_config.contains(MANAGED_CONFIG_MARKER)
}

fn managed_backup_path(config_path: &std::path::Path) -> PathBuf {
    sibling_path_with_extra_extension(config_path, "codex-mixin.backup")
}

fn managed_absent_marker_path(config_path: &std::path::Path) -> PathBuf {
    sibling_path_with_extra_extension(config_path, "codex-mixin.absent")
}

fn sibling_path_with_extra_extension(config_path: &std::path::Path, suffix: &str) -> PathBuf {
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    config_path.with_file_name(format!("{file_name}.{suffix}"))
}

fn default_codex_config_path() -> PathBuf {
    codex_home_path().join("config.toml")
}

fn default_codex_catalog_path() -> PathBuf {
    codex_home_path()
        .join("model-catalogs")
        .join("mixin-models.json")
}

fn codex_home_path() -> PathBuf {
    std::env::var("CODEX_HOME").ok().map_or_else(
        || {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".codex")
        },
        PathBuf::from,
    )
}

fn validate_provider_name(provider: &str) -> anyhow::Result<()> {
    if provider.is_empty() {
        anyhow::bail!("provider cannot be empty");
    }
    if provider == "openai" {
        anyhow::bail!("provider 'openai' is reserved by Codex; use 'custom'");
    }
    if provider
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Ok(());
    }
    anyhow::bail!("provider may only contain ASCII letters, digits, underscore, or hyphen")
}

fn select_codex_model(
    requested_model: Option<String>,
    models: &[codex_mixin::anthropic::ModelInfo],
    doc: &DocumentMut,
) -> anyhow::Result<String> {
    if let Some(model) = requested_model {
        if models.iter().any(|candidate| candidate.id == model) {
            return Ok(model);
        }
        anyhow::bail!("requested model is not present in upstream /v1/models: {model}");
    }
    if let Some(current_model) = doc.get("model").and_then(Item::as_str)
        && models.iter().any(|candidate| candidate.id == current_model)
    {
        return Ok(current_model.to_owned());
    }
    if let Some(model) = models.iter().find(|model| model.id == "Claude Sonnet 5") {
        return Ok(model.id.clone());
    }
    Ok(models[0].id.clone())
}

fn select_codex_oauth_proxy_model(
    requested_model: Option<String>,
    models: &[codex_mixin::anthropic::ModelInfo],
    template_catalog: Option<&serde_json::Value>,
    doc: &DocumentMut,
) -> anyhow::Result<String> {
    if let Some(model) = requested_model {
        if model_exists_in_oauth_proxy_catalog(&model, models, template_catalog) {
            return Ok(model);
        }
        if let Some(canonical) = model.strip_suffix("-custom")
            && is_gpt_model(canonical)
            && models.iter().any(|candidate| candidate.id == canonical)
        {
            return Ok(model);
        }
        if is_gpt_model(&model) && models.iter().any(|candidate| candidate.id == model) {
            return Ok(format!("{model}-custom"));
        }
        anyhow::bail!("requested model is not present in generated Codex catalog: {model}");
    }
    if let Some(current_model) = doc.get("model").and_then(Item::as_str)
        && model_exists_in_oauth_proxy_catalog(current_model, models, template_catalog)
    {
        return Ok(current_model.to_owned());
    }
    for preferred in ["gpt-5.5", "gpt-5.4", "gpt-5.4-mini"] {
        if template_catalog_has_model(template_catalog, preferred) {
            return Ok(preferred.to_owned());
        }
    }
    if let Some(model) = template_catalog
        .and_then(|catalog| catalog.get("models"))
        .and_then(serde_json::Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
                .find(|slug| is_gpt_model(slug))
        })
    {
        return Ok(model.to_owned());
    }
    if let Some(model) = models.iter().find(|model| model.id == "Claude Sonnet 5") {
        return Ok(model.id.clone());
    }
    let first = &models[0].id;
    if is_gpt_model(first) {
        Ok(format!("{first}-custom"))
    } else {
        Ok(first.clone())
    }
}

fn model_exists_in_oauth_proxy_catalog(
    model: &str,
    models: &[codex_mixin::anthropic::ModelInfo],
    template_catalog: Option<&serde_json::Value>,
) -> bool {
    if template_catalog_has_model(template_catalog, model) {
        return true;
    }
    if let Some(canonical) = model.strip_suffix("-custom")
        && is_gpt_model(canonical)
    {
        return models.iter().any(|candidate| candidate.id == canonical);
    }
    models
        .iter()
        .any(|candidate| candidate.id == model && !is_gpt_model(&candidate.id))
}

fn template_catalog_has_model(template_catalog: Option<&serde_json::Value>, slug: &str) -> bool {
    template_catalog
        .and_then(|catalog| catalog.get("models"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|models| {
            models
                .iter()
                .any(|model| model.get("slug").and_then(serde_json::Value::as_str) == Some(slug))
        })
}

fn is_gpt_model(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("gpt-")
}

fn upsert_codex_config(
    doc: &mut DocumentMut,
    provider: &str,
    default_model: Option<&str>,
    catalog_path: &std::path::Path,
    base_url: &str,
    web_search: &str,
    env_key: Option<&str>,
    codex_oauth_proxy: bool,
) -> anyhow::Result<()> {
    doc["model_catalog_json"] = value(catalog_path.to_string_lossy().to_string());
    doc["model_provider"] = value(provider);
    if let Some(model) = default_model {
        doc["model"] = value(model);
        doc["web_search"] = value(web_search);
    }

    if !doc
        .get("model_providers")
        .is_some_and(|item| item.is_table())
    {
        doc["model_providers"] = Item::Table(Table::new());
    }
    let providers = doc["model_providers"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("model_providers must be a TOML table"))?;
    if !providers.get(provider).is_some_and(|item| item.is_table()) {
        providers.insert(provider, Item::Table(Table::new()));
    }
    let provider_table = providers
        .get_mut(provider)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("model provider entry must be a TOML table"))?;
    provider_table["name"] = value(if codex_oauth_proxy {
        "OpenAI"
    } else {
        "Custom models through local Responses gateway"
    });
    provider_table["base_url"] = value(base_url);
    provider_table["wire_api"] = value("responses");
    if codex_oauth_proxy {
        provider_table["requires_openai_auth"] = value(true);
        provider_table["supports_websockets"] = value(true);
        provider_table.remove("env_key");
    } else {
        provider_table.remove("requires_openai_auth");
        provider_table.remove("supports_websockets");
        if let Some(env_key) = env_key {
            provider_table["env_key"] = value(env_key);
        } else {
            provider_table.remove("env_key");
        }
    }
    Ok(())
}

async fn start(
    bind: Option<SocketAddr>,
    daemon: bool,
    log_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let mut config = GatewayConfig::from_env()?;
    if let Some(bind) = bind {
        config.bind = bind;
    }
    if daemon {
        return start_daemon(config.bind, log_file);
    }
    refresh_default_managed_codex_catalog()?;
    serve(config).await
}

fn start_daemon(bind: SocketAddr, log_file: Option<PathBuf>) -> anyhow::Result<()> {
    if let Some(metadata) = load_daemon_metadata()?
        && pid_is_running(metadata.pid)?
    {
        anyhow::bail!(
            "gateway daemon already running: pid {}, bind {}",
            metadata.pid,
            metadata.bind
        );
    }
    let log_file = log_file.unwrap_or_else(default_log_file_path);
    if let Some(parent) = log_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)?;
    let stderr = stdout.try_clone()?;
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command
        .arg("start")
        .arg("--bind")
        .arg(bind.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    let pid = child.id();
    thread::sleep(Duration::from_millis(300));
    if !pid_is_running(pid)? {
        anyhow::bail!(
            "gateway daemon exited immediately; inspect log: {}",
            log_file.display()
        );
    }
    save_daemon_metadata(&DaemonMetadata {
        pid,
        bind,
        log_file: log_file.clone(),
        started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    })?;
    println!("gateway daemon started");
    println!("pid: {pid}");
    println!("bind: {bind}");
    println!("log: {}", log_file.display());
    Ok(())
}

fn stop(force: bool) -> anyhow::Result<()> {
    let Some(metadata) = load_daemon_metadata()? else {
        println!("gateway daemon is not recorded");
        return Ok(());
    };
    if !pid_is_running(metadata.pid)? {
        delete_daemon_metadata()?;
        println!("removed stale daemon metadata for pid {}", metadata.pid);
        return Ok(());
    }
    send_signal(metadata.pid, "TERM")?;
    for _ in 0..50 {
        if !pid_is_running(metadata.pid)? {
            delete_daemon_metadata()?;
            println!("gateway daemon stopped: pid {}", metadata.pid);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    if force {
        send_signal(metadata.pid, "KILL")?;
        delete_daemon_metadata()?;
        println!("gateway daemon killed: pid {}", metadata.pid);
        return Ok(());
    }
    anyhow::bail!(
        "gateway daemon did not stop within 5s: pid {}. Use --force to send SIGKILL",
        metadata.pid
    )
}

async fn restart(bind: Option<SocketAddr>, log_file: Option<PathBuf>) -> anyhow::Result<()> {
    stop(false)?;
    start(bind, true, log_file).await
}

fn logs(lines: usize, follow: bool) -> anyhow::Result<()> {
    let log_file = load_daemon_metadata()?
        .map(|metadata| metadata.log_file)
        .unwrap_or_else(default_log_file_path);
    if !log_file.exists() {
        anyhow::bail!("log file does not exist: {}", log_file.display());
    }
    if follow {
        let status = ProcessCommand::new("tail")
            .arg("-n")
            .arg(lines.to_string())
            .arg("-f")
            .arg(&log_file)
            .status()?;
        if !status.success() {
            anyhow::bail!("tail exited with status {status}");
        }
        return Ok(());
    }
    let content = fs::read_to_string(&log_file)?;
    let lines = content.lines().rev().take(lines).collect::<Vec<_>>();
    for line in lines.into_iter().rev() {
        println!("{line}");
    }
    Ok(())
}

fn state_dir() -> PathBuf {
    stored_config_path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn daemon_metadata_path() -> PathBuf {
    std::env::var("CODEX_GATEWAY_DAEMON_FILE")
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir().join("daemon.json"))
}

fn default_log_file_path() -> PathBuf {
    std::env::var("CODEX_GATEWAY_LOG_FILE")
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir().join("gateway.log"))
}

fn load_daemon_metadata() -> anyhow::Result<Option<DaemonMetadata>> {
    let path = daemon_metadata_path();
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

fn save_daemon_metadata(metadata: &DaemonMetadata) -> anyhow::Result<()> {
    let path = daemon_metadata_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
}

fn delete_daemon_metadata() -> anyhow::Result<()> {
    let path = daemon_metadata_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn pid_is_running(pid: u32) -> anyhow::Result<bool> {
    let status = ProcessCommand::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
}

fn send_signal(pid: u32, signal: &str) -> anyhow::Result<()> {
    let status = ProcessCommand::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()?;
    if !status.success() {
        anyhow::bail!("failed to send SIG{signal} to pid {pid}");
    }
    Ok(())
}

fn prompt_required(label: &str) -> anyhow::Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    trim_required(label, value)
}

fn trim_required(label: &str, value: String) -> anyhow::Result<String> {
    let trimmed = value.trim().to_owned();
    if trimmed.is_empty() {
        anyhow::bail!("{label} cannot be empty");
    }
    Ok(trimmed)
}

fn first_env_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

fn normalize_base_url(value: String) -> anyhow::Result<String> {
    let mut trimmed = trim_required("base URL", value)?;
    while trimmed.ends_with('/') {
        trimmed.pop();
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        anyhow::bail!("base URL must start with http:// or https://");
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_install_backup_and_uninstall_restore_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let catalog_path = dir.path().join("model-catalogs").join("mixin-models.json");
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        fs::write(&config_path, "model = \"gpt-5.5\"\n").unwrap();
        fs::write(&catalog_path, "{}").unwrap();

        let original = prepare_managed_config_install(&config_path).unwrap();
        assert_eq!(original, "model = \"gpt-5.5\"\n");
        assert!(managed_backup_path(&config_path).exists());
        fs::write(
            &config_path,
            format!("{MANAGED_CONFIG_HEADER}\nmodel = \"Claude Sonnet 5\"\n"),
        )
        .unwrap();

        uninstall_codex(Some(config_path.clone()), Some(catalog_path.clone())).unwrap();
        assert_eq!(
            fs::read_to_string(&config_path).unwrap(),
            "model = \"gpt-5.5\"\n"
        );
        assert!(!managed_backup_path(&config_path).exists());
        assert!(!catalog_path.exists());
    }

    #[test]
    fn managed_uninstall_removes_config_when_none_existed_before() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let catalog_path = dir.path().join("model-catalogs").join("mixin-models.json");

        let original = prepare_managed_config_install(&config_path).unwrap();
        assert!(original.is_empty());
        assert!(managed_absent_marker_path(&config_path).exists());
        fs::write(&config_path, format!("{MANAGED_CONFIG_HEADER}\n")).unwrap();

        uninstall_codex(Some(config_path.clone()), Some(catalog_path)).unwrap();
        assert!(!config_path.exists());
        assert!(!managed_absent_marker_path(&config_path).exists());
    }

    #[test]
    fn oauth_proxy_install_registers_custom_provider() {
        let mut doc = r#"
model_provider = "openai"
model = "gpt-5.5"

[model_providers.openai]
name = "OpenAI"
base_url = "https://chatgpt.com/backend-api/codex"
wire_api = "responses"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let catalog_path = PathBuf::from("/tmp/mixin-models.json");
        let provider = "custom";

        upsert_codex_config(
            &mut doc,
            &provider,
            None,
            &catalog_path,
            "http://127.0.0.1:8787/v1",
            "disabled",
            None,
            true,
        )
        .unwrap();

        assert_eq!(doc["model_provider"].as_str(), Some("custom"));
        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(
            doc["model_providers"]["custom"]["base_url"].as_str(),
            Some("http://127.0.0.1:8787/v1")
        );
        assert_eq!(
            doc["model_providers"]["custom"]["requires_openai_auth"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn oauth_proxy_install_writes_custom_provider_without_default_model() {
        let mut doc = "model = \"gpt-5.5\"\n".parse::<DocumentMut>().unwrap();
        let catalog_path = PathBuf::from("/tmp/mixin-models.json");
        let provider = "custom";

        upsert_codex_config(
            &mut doc,
            &provider,
            None,
            &catalog_path,
            "http://127.0.0.1:8787/v1",
            "disabled",
            None,
            true,
        )
        .unwrap();

        assert_eq!(doc["model_provider"].as_str(), Some("custom"));
        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(
            doc["model_providers"]["custom"]["base_url"].as_str(),
            Some("http://127.0.0.1:8787/v1")
        );
    }

    #[test]
    fn refreshes_managed_catalog_from_latest_official_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let official_path = dir.path().join("models_cache.json");
        let catalog_path = dir.path().join("model-catalogs").join("mixin-models.json");
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            format!(
                "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n",
                catalog_path.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            &official_path,
            r#"{"models":[{"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol"}]}"#,
        )
        .unwrap();
        fs::write(
            &catalog_path,
            r#"{"models":[{"slug":"gpt-5.5","display_name":"GPT-5.5"},{"slug":"DeepSeek-V4-Flash","description":"Custom upstream model exposed through codex-mixin"}]}"#,
        )
        .unwrap();

        assert!(refresh_managed_codex_catalog(&config_path, &official_path).unwrap());
        let refreshed: serde_json::Value =
            serde_json::from_slice(&fs::read(&catalog_path).unwrap()).unwrap();
        assert_eq!(refreshed["models"][0]["slug"], "gpt-5.6-sol");
        assert_eq!(refreshed["models"][1]["slug"], "DeepSeek-V4-Flash");
        assert_eq!(refreshed["models"][1]["multi_agent_version"], "v2");
        assert!(!refresh_managed_codex_catalog(&config_path, &official_path).unwrap());
    }

    #[test]
    fn summarizes_generic_quota_shapes() {
        assert_eq!(
            summarize_quota_json(&serde_json::json!({"usage":{"used":"12.5","budget":100}})),
            "quota used: 12.5 / 100"
        );
        assert_eq!(
            summarize_quota_json(&serde_json::json!({"data":{"used":42}})),
            "quota used: 42"
        );
        assert_eq!(
            summarize_quota_json(
                &serde_json::json!({"data":{"used_quota":10,"month_quota_limit":50,"remaining_quota":40}})
            ),
            "quota used: 10 / 50, remaining: 40"
        );
    }

    #[test]
    fn provider_presets_resolve_quota_urls() {
        let baidu = StoredGatewayConfig {
            provider_preset: Some("baidu-oneapi".to_owned()),
            upstream_base_url: Some("https://oneapi.example".to_owned()),
            ..StoredGatewayConfig::default()
        };
        assert_eq!(
            resolve_quota_url(&baidu).unwrap().as_str(),
            "https://oneapi.example/openapi/v3/user/quota"
        );

        let openrouter = StoredGatewayConfig {
            provider_preset: Some("openrouter".to_owned()),
            ..StoredGatewayConfig::default()
        };
        assert_eq!(
            resolve_quota_url(&openrouter).unwrap().as_str(),
            "https://openrouter.ai/api/v1/credits"
        );

        let deepseek = StoredGatewayConfig {
            provider_preset: Some("deepseek".to_owned()),
            ..StoredGatewayConfig::default()
        };
        assert!(resolve_quota_url(&deepseek).is_err());
    }
}
