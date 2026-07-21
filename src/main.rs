#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, value};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use codex_mixin::CODEX_MIXIN_PROVIDER;
use codex_mixin::catalog::{
    apply_web_search_capabilities, codex_catalog_from_models_with_metadata,
    codex_oauth_proxy_catalog_from_models_with_metadata, load_template_catalog,
    refresh_managed_oauth_catalog,
};
use codex_mixin::config::{
    GatewayConfig, ProviderPreset, StoredGatewayConfig, delete_stored_config, load_stored_config,
    save_stored_config, stored_config_path,
};
use codex_mixin::history::{
    migrate_history_from_mixin_provider, migrate_history_to_mixin_provider,
};
use codex_mixin::model_metadata::{ModelMetadataResolver, default_cache_path};
use codex_mixin::server::{AppState, serve_on_listener};
use codex_mixin::web_search::WebSearchCapabilities;

const MANAGED_CONFIG_MARKER: &str = "codex-mixin managed config";
const MANAGED_CONFIG_HEADER: &str = "# codex-mixin managed config. Run `codex-mixin uninstall-codex` to restore the previous config.";
const LITELLM_MODEL_METADATA_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const CODEX_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const GATEWAY_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Eq, PartialEq)]
struct CodexInstallPaths {
    config: PathBuf,
    catalog: PathBuf,
    models_cache: PathBuf,
}

struct ManagedConfigLock {
    #[cfg(unix)]
    _file: fs::File,
}

impl ManagedConfigLock {
    fn acquire(config_path: &Path) -> anyhow::Result<Self> {
        #[cfg(not(unix))]
        {
            let _ = config_path;
            anyhow::bail!("managed Codex config locking requires Unix flock support");
        }

        #[cfg(unix)]
        {
            if let Some(parent) = config_path.parent() {
                fs::create_dir_all(parent)?;
            }
            // A sibling file keeps the lock inode stable while config writes use atomic rename.
            let lock_path = sibling_path_with_extra_extension(config_path, "codex-mixin.lock");
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)?;
            file.lock().map_err(|error| {
                anyhow::anyhow!(
                    "failed to lock managed Codex config {}: {error}",
                    config_path.display()
                )
            })?;
            Ok(Self { _file: file })
        }
    }
}

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
        image_generation_path: Option<String>,
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
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        catalog: Option<PathBuf>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long, default_value = "live")]
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
    #[command(name = "probe-web-search")]
    ProbeWebSearch {
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RuntimeMetadata {
    pid: u32,
    bind: SocketAddr,
    started_at: u64,
    #[serde(default)]
    version: Option<String>,
}

struct RuntimeMetadataGuard {
    pid: u32,
}

impl Drop for RuntimeMetadataGuard {
    fn drop(&mut self) {
        if load_runtime_metadata()
            .ok()
            .flatten()
            .is_some_and(|metadata| metadata.pid == self.pid)
        {
            let _ = delete_runtime_metadata();
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let foreground_log_file = match &cli.command {
        Some(Command::Start {
            daemon: false,
            log_file: Some(path),
            ..
        }) => Some(path.clone()),
        _ => None,
    };
    if let Err(error) = init_tracing(foreground_log_file.as_deref()) {
        eprintln!("Error: failed to initialize logging: {error:#}");
        std::process::exit(1);
    }
    if foreground_log_file.is_some() {
        tracing::info!(
            version = env!("CARGO_PKG_VERSION"),
            pid = std::process::id(),
            "gateway process starting"
        );
    }
    if let Err(error) = run(cli).await {
        if foreground_log_file.is_some() {
            tracing::error!(error = %format!("{error:#}"), "command failed");
        } else {
            eprintln!("Error: {error:#}");
        }
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command.unwrap_or(Command::Start {
        bind: None,
        daemon: false,
        log_file: None,
    }) {
        Command::Login {
            provider,
            key,
            base_url,
            image_generation_path,
            gateway_key,
            quota_url,
            quota_username,
        } => login(
            provider,
            key,
            base_url,
            image_generation_path,
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
            let mut models = state.fetch_models().await?;
            state
                .probe_web_search_capabilities(&mut models, false)
                .await?;
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
            config,
            catalog,
            base_url,
            web_search,
            env_key,
            no_env_key,
        } => {
            install_codex(InstallCodexOptions {
                requested_model: model,
                set_default,
                codex_oauth_proxy,
                config_path: config,
                catalog_path: catalog,
                base_url,
                web_search,
                env_key,
                no_env_key,
            })
            .await
        }
        Command::UninstallCodex { config, catalog } => uninstall_codex(config, catalog),
        Command::RefreshCodexCatalog => refresh_default_managed_codex_catalog(),
        Command::ProbeWebSearch { force, json } => probe_web_search(force, json).await,
    }
}

fn init_tracing(log_file: Option<&Path>) -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();
    if let Some(log_file) = log_file {
        rotate_gateway_log_if_needed(log_file, GATEWAY_LOG_MAX_BYTES)?;
        if let Some(parent) = log_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)?;
        #[cfg(unix)]
        fs::set_permissions(log_file, fs::Permissions::from_mode(0o600))?;
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter(filter)
            .with_writer(Mutex::new(file))
            .try_init()
            .map_err(|error| anyhow::anyhow!("failed to install tracing subscriber: {error}"))?;
    } else {
        tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .with_env_filter(filter)
            .try_init()
            .map_err(|error| anyhow::anyhow!("failed to install tracing subscriber: {error}"))?;
    }
    Ok(())
}

fn rotate_gateway_log_if_needed(path: &Path, max_bytes: u64) -> anyhow::Result<()> {
    if !path.exists() || fs::metadata(path)?.len() < max_bytes {
        return Ok(());
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let mut backup_name = path.as_os_str().to_os_string();
    backup_name.push(".1");
    let backup = PathBuf::from(backup_name);
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    fs::rename(path, backup)?;
    Ok(())
}

fn login(
    provider: Option<String>,
    key: Option<String>,
    base_url: Option<String>,
    image_generation_path: Option<String>,
    gateway_key: Option<String>,
    quota_url: Option<String>,
    quota_username: Option<String>,
) -> anyhow::Result<()> {
    let explicit_key = key.is_some();
    let existing = load_stored_config()?.unwrap_or_default();
    let provider_preset = provider
        .or(existing.provider_preset.clone())
        .or_else(|| first_env_value(&["CODEX_GATEWAY_PROVIDER"]))
        .map(|value| ProviderPreset::parse(&value))
        .transpose()?
        .unwrap_or(ProviderPreset::Custom);
    let provider_changed = existing.provider_preset.as_deref() != Some(provider_preset.as_str());
    let upstream_api_key = match key {
        Some(key) => trim_required("key", key)?,
        None => existing
            .upstream_api_key
            .clone()
            .map(Ok)
            .unwrap_or_else(|| prompt_required("upstream API key"))?,
    };
    let upstream_base_url = provider_preset.normalize_upstream_base_url(normalize_base_url(
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
    )?);
    let upstream_kind = provider_preset.default_upstream_kind();
    let config = StoredGatewayConfig {
        gateway_bind: existing.gateway_bind,
        provider_preset: Some(provider_preset.as_str().to_owned()),
        upstream_kind: Some(upstream_kind.as_str().to_owned()),
        upstream_base_url: Some(upstream_base_url.clone()),
        upstream_messages_path: Some(provider_preset.default_messages_path().to_owned()),
        upstream_models_path: Some(provider_preset.default_models_path().to_owned()),
        upstream_image_generation_path: resolve_image_generation_path(
            image_generation_path,
            provider_changed,
            existing.upstream_image_generation_path,
            provider_preset.default_image_generation_path(),
        ),
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
        fusion_profiles: existing.fusion_profiles,
    };
    let path = save_stored_config(&config)?;
    if provider_changed || explicit_key {
        WebSearchCapabilities::clear_default_cache()?;
    }
    println!("login saved: {}", path.display());
    println!("provider: {}", provider_preset.as_str());
    println!("upstream kind: {}", upstream_kind.as_str());
    println!("upstream: {upstream_base_url}");
    println!(
        "upstream image generation: {}",
        config
            .upstream_image_generation_path
            .as_deref()
            .filter(|path| !path.is_empty())
            .unwrap_or("not configured")
    );
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

fn resolve_image_generation_path(
    explicit: Option<String>,
    provider_changed: bool,
    existing: Option<String>,
    provider_default: Option<&str>,
) -> Option<String> {
    match explicit {
        // Persist an empty string as an explicit opt-out so preset defaults stay disabled.
        Some(path) if path.trim().is_empty() => Some(String::new()),
        Some(path) => Some(path.trim().to_owned()),
        None if !provider_changed => existing,
        None => provider_default.map(str::to_owned),
    }
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

async fn probe_web_search(force: bool, json_output: bool) -> anyhow::Result<()> {
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
    if !url.query_pairs().any(|(key, _)| key == "username")
        && let Some(username) = quota_username(stored)
    {
        url.query_pairs_mut().append_pair("username", &username);
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
    let outcome = migrate_history_to_mixin_provider(&codex_home)?;
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
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("metadata endpoint returned {status}: {body}");
    }
    Ok(body)
}

struct InstallCodexOptions {
    requested_model: Option<String>,
    set_default: bool,
    codex_oauth_proxy: bool,
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
    base_url: Option<String>,
    web_search: String,
    env_key: Option<String>,
    no_env_key: bool,
}

async fn install_codex(options: InstallCodexOptions) -> anyhow::Result<()> {
    let InstallCodexOptions {
        requested_model,
        set_default,
        codex_oauth_proxy,
        config_path,
        catalog_path,
        base_url,
        web_search,
        env_key,
        no_env_key,
    } = options;
    let paths = resolve_codex_install_paths(config_path, catalog_path)?;
    let template = load_codex_install_template(&paths, codex_oauth_proxy)?;
    let gateway_config = GatewayConfig::from_env()?;
    let state = AppState::new(gateway_config.clone())?;
    let mut models = state.fetch_models().await?;
    if models.is_empty() {
        anyhow::bail!("upstream /v1/models returned no models");
    }
    let web_search_probe = state
        .probe_web_search_capabilities(&mut models, false)
        .await?;
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
    let serialized_catalog = serde_json::to_vec_pretty(&catalog)?;
    let gateway_bind = match load_runtime_metadata()? {
        Some(runtime) if pid_is_running(runtime.pid)? => runtime.bind,
        _ => gateway_config.bind,
    };
    let gateway_base_url =
        normalize_base_url(base_url.unwrap_or_else(|| format!("http://{gateway_bind}/v1")))?;
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

    let _config_lock = ManagedConfigLock::acquire(&paths.config)?;
    let raw_config = read_managed_config_for_install(&paths.config)?;
    let mut doc = if raw_config.trim().is_empty() {
        DocumentMut::new()
    } else {
        raw_config.parse::<DocumentMut>()?
    };
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
    upsert_codex_config(
        &mut doc,
        selected_model.as_deref(),
        &paths.catalog,
        &gateway_base_url,
        &web_search,
        env_key.as_deref(),
        codex_oauth_proxy,
    )?;
    let serialized_config = format!("{MANAGED_CONFIG_HEADER}\n{doc}");
    serialized_config.parse::<DocumentMut>()?;
    let expected_model_slugs = catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("generated Codex catalog has no models array"))?
        .iter()
        .map(|model| {
            model
                .get("slug")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("generated Codex model is missing slug"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let codex_home = paths
        .config
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?;
    write_managed_codex_files(
        &paths,
        &raw_config,
        &serialized_catalog,
        serialized_config.as_bytes(),
        || validate_codex_install(codex_home, &expected_model_slugs),
    )?;

    let history = migrate_history_to_mixin_provider(codex_home)?;
    println!("codex config updated: {}", paths.config.display());
    println!(
        "codex config backup: {}",
        managed_backup_path(&paths.config).display()
    );
    println!("model catalog written: {}", paths.catalog.display());
    println!("models installed: {}", models.len());
    println!("metadata entries loaded: {}", metadata.len());
    println!(
        "web search capabilities: {} supported, {} unsupported, {} failed",
        web_search_probe.supported, web_search_probe.unsupported, web_search_probe.failed
    );
    println!("provider: {CODEX_MIXIN_PROVIDER}");
    println!(
        "history migrated: {} JSONL files, {} SQLite rows",
        history.jsonl_files_changed, history.sqlite_rows_changed
    );
    if let Some(backup_root) = history.backup_root {
        println!("history backup: {}", backup_root.display());
    }
    if codex_oauth_proxy {
        println!("codex oauth proxy: enabled");
    }
    if let Some(selected_model) = selected_model {
        println!("default model: {selected_model}");
    } else {
        println!("default model: unchanged");
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
    let config_path = resolve_codex_config_path(config_path)?;
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
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
    let managed_doc = raw_config.parse::<DocumentMut>()?;
    let managed_catalog_path = managed_catalog_path(&managed_doc, &config_path)?;
    if let Some(explicit_catalog_path) = catalog_path {
        let explicit_catalog_path = absolute_path(explicit_catalog_path)?;
        if explicit_catalog_path != managed_catalog_path {
            anyhow::bail!(
                "explicit catalog {} does not match managed config catalog {}",
                explicit_catalog_path.display(),
                managed_catalog_path.display()
            );
        }
    }
    let backup_path = managed_backup_path(&config_path);
    let absent_marker_path = managed_absent_marker_path(&config_path);
    let restored_provider = if backup_path.exists() {
        let backup = fs::read_to_string(&backup_path)?;
        let doc = backup.parse::<DocumentMut>()?;
        doc.get("model_provider")
            .and_then(Item::as_str)
            .unwrap_or("openai")
            .to_owned()
    } else if absent_marker_path.exists() {
        "openai".to_owned()
    } else {
        anyhow::bail!(
            "missing managed backup for {}; expected {}",
            config_path.display(),
            backup_path.display()
        );
    };
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
    }
    let codex_home = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?;
    let history = migrate_history_from_mixin_provider(codex_home, &restored_provider)?;
    println!("history provider restored: {restored_provider}");
    println!(
        "history restored: {} JSONL files, {} SQLite rows",
        history.jsonl_files_changed, history.sqlite_rows_changed
    );
    if let Some(backup_root) = history.backup_root {
        println!("history backup: {}", backup_root.display());
    }
    if managed_catalog_path.exists() {
        fs::remove_file(&managed_catalog_path)?;
        println!("model catalog removed: {}", managed_catalog_path.display());
    }
    println!("reload required: restart Codex app; for Codex CLI, start a new session");
    Ok(())
}

fn refresh_default_managed_codex_catalog() -> anyhow::Result<()> {
    let config_path = resolve_codex_config_path(None)?;
    let gateway_config = GatewayConfig::from_env()?;
    let supported_models =
        WebSearchCapabilities::from_default_path(&gateway_config)?.supported_model_ids();
    if refresh_managed_codex_catalog_with_capabilities(&config_path, Some(&supported_models))? {
        println!("Codex model catalog refreshed");
    } else {
        println!("Codex model catalog already current or not managed by codex-mixin");
    }
    Ok(())
}

#[cfg(test)]
fn refresh_managed_codex_catalog(config_path: &Path) -> anyhow::Result<bool> {
    refresh_managed_codex_catalog_with_capabilities(config_path, None)
}

fn refresh_managed_codex_catalog_with_capabilities(
    config_path: &Path,
    supported_web_search_models: Option<&HashSet<String>>,
) -> anyhow::Result<bool> {
    let config_path = absolute_path(config_path.to_path_buf())?;
    if !config_path.exists() {
        return Ok(false);
    }
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
    if !config_path.exists() {
        return Ok(false);
    }
    let raw_config = fs::read_to_string(&config_path)?;
    if !is_managed_config(&raw_config) {
        return Ok(false);
    }
    let doc = raw_config.parse::<DocumentMut>()?;
    let provider = doc
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(CODEX_MIXIN_PROVIDER))
        .and_then(Item::as_table)
        .ok_or_else(|| anyhow::anyhow!("managed Codex config has no codex-mixin provider"))?;
    let requires_openai_auth = match provider.get("requires_openai_auth") {
        Some(item) => item
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("codex-mixin requires_openai_auth must be a boolean"))?,
        None => false,
    };
    if !requires_openai_auth && supported_web_search_models.is_none() {
        return Ok(false);
    }
    let catalog_path = managed_catalog_path(&doc, &config_path)?;
    let managed_catalog = serde_json::from_slice(&fs::read(&catalog_path)?)?;
    let mut refreshed = if requires_openai_auth {
        let official_catalog_path = config_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?
            .join("models_cache.json");
        let official_catalog = serde_json::from_slice(&fs::read(&official_catalog_path)?)?;
        refresh_managed_oauth_catalog(&official_catalog, &managed_catalog)?
    } else {
        managed_catalog
    };
    if let Some(supported_web_search_models) = supported_web_search_models {
        apply_web_search_capabilities(&mut refreshed, supported_web_search_models)?;
    }
    write_atomic_if_changed(&catalog_path, &serde_json::to_vec_pretty(&refreshed)?)
}

fn write_atomic_if_changed(path: &Path, contents: &[u8]) -> anyhow::Result<bool> {
    if path.exists() && fs::read(path)? == contents {
        return Ok(false);
    }
    let existing_permissions = if path.exists() {
        Some(fs::metadata(path)?.permissions())
    } else {
        None
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model-catalog.json");
    let temporary_path = path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    fs::write(&temporary_path, contents)?;
    if let Some(permissions) = existing_permissions {
        fs::set_permissions(&temporary_path, permissions)?;
    }
    if let Err(err) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(err.into());
    }
    Ok(true)
}

fn resolve_codex_install_paths(
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
) -> anyhow::Result<CodexInstallPaths> {
    let config = resolve_codex_config_path(config_path)?;
    let codex_home = config
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?
        .to_path_buf();
    let catalog = match catalog_path {
        Some(path) => absolute_path(path)?,
        None => codex_home.join("model-catalogs").join("mixin-models.json"),
    };
    Ok(CodexInstallPaths {
        config,
        catalog,
        models_cache: codex_home.join("models_cache.json"),
    })
}

fn resolve_codex_config_path(config_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    absolute_path(config_path.unwrap_or_else(default_codex_config_path))
}

fn absolute_path(path: PathBuf) -> anyhow::Result<PathBuf> {
    Ok(std::path::absolute(path)?)
}

fn load_codex_install_template(
    paths: &CodexInstallPaths,
    codex_oauth_proxy: bool,
) -> anyhow::Result<Option<serde_json::Value>> {
    let template = load_template_catalog(Some(&paths.models_cache))?;
    if codex_oauth_proxy && template.is_none() {
        anyhow::bail!(
            "official Codex model cache is missing: {}. Open Codex once before installing Codex Mixin",
            paths.models_cache.display()
        );
    }
    Ok(template)
}

fn write_managed_codex_files(
    paths: &CodexInstallPaths,
    raw_config: &str,
    serialized_catalog: &[u8],
    serialized_config: &[u8],
    validate: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let config_existed = paths.config.exists();
    let previous_catalog = if paths.catalog.exists() {
        Some(fs::read(&paths.catalog)?)
    } else {
        None
    };
    let created_restore_point = !is_managed_config(raw_config);
    create_managed_config_restore_point(&paths.config, raw_config)?;
    let install_result = (|| -> anyhow::Result<()> {
        write_atomic_if_changed(&paths.catalog, serialized_catalog)?;
        write_atomic_if_changed(&paths.config, serialized_config)?;
        validate()
    })();
    let Err(install_error) = install_result else {
        return Ok(());
    };

    let mut rollback_errors = Vec::new();
    let config_rollback = if config_existed {
        write_atomic_if_changed(&paths.config, raw_config.as_bytes()).map(|_| ())
    } else if paths.config.exists() {
        fs::remove_file(&paths.config).map_err(Into::into)
    } else {
        Ok(())
    };
    if let Err(error) = config_rollback {
        rollback_errors.push(format!("restore config: {error}"));
    }
    let catalog_rollback = match previous_catalog {
        Some(previous_catalog) => {
            write_atomic_if_changed(&paths.catalog, &previous_catalog).map(|_| ())
        }
        None if paths.catalog.exists() => fs::remove_file(&paths.catalog).map_err(Into::into),
        None => Ok(()),
    };
    if let Err(error) = catalog_rollback {
        rollback_errors.push(format!("restore catalog: {error}"));
    }
    if created_restore_point {
        for restore_path in [
            managed_backup_path(&paths.config),
            managed_absent_marker_path(&paths.config),
        ] {
            if restore_path.exists()
                && let Err(error) = fs::remove_file(&restore_path)
            {
                rollback_errors.push(format!(
                    "remove restore point {}: {error}",
                    restore_path.display()
                ));
            }
        }
    }
    if rollback_errors.is_empty() {
        anyhow::bail!(
            "Codex rejected the managed configuration; installation rolled back: {install_error}"
        );
    }
    anyhow::bail!(
        "Codex rejected the managed configuration: {install_error}; rollback also failed: {}",
        rollback_errors.join("; ")
    )
}

fn validate_codex_install(
    codex_home: &Path,
    expected_model_slugs: &[String],
) -> anyhow::Result<()> {
    let codex_cli = resolve_codex_cli()?;
    let doctor = ProcessCommand::new(&codex_cli)
        .args(["doctor", "--json"])
        .env("CODEX_HOME", codex_home)
        .output()?;
    let doctor_report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).map_err(|error| {
            anyhow::anyhow!(
                "Codex doctor returned invalid JSON: {error}; stderr: {}",
                String::from_utf8_lossy(&doctor.stderr)
                    .chars()
                    .take(1000)
                    .collect::<String>()
            )
        })?;
    let config_check = doctor_report
        .pointer("/checks/config.load")
        .ok_or_else(|| anyhow::anyhow!("Codex doctor report has no config.load check"))?;
    if config_check
        .get("status")
        .and_then(serde_json::Value::as_str)
        != Some("ok")
    {
        anyhow::bail!("Codex config.load check failed: {config_check}");
    }
    let effective_provider = config_check
        .pointer("/details/model provider")
        .and_then(serde_json::Value::as_str);
    if effective_provider != Some(CODEX_MIXIN_PROVIDER) {
        anyhow::bail!(
            "Codex loaded model provider {:?}, expected {CODEX_MIXIN_PROVIDER}",
            effective_provider
        );
    }

    let models = ProcessCommand::new(&codex_cli)
        .args(["debug", "models"])
        .env("CODEX_HOME", codex_home)
        .output()?;
    if !models.status.success() {
        anyhow::bail!(
            "Codex failed to load the managed model catalog: {}",
            String::from_utf8_lossy(&models.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
        );
    }
    let loaded_catalog: serde_json::Value = serde_json::from_slice(&models.stdout)
        .map_err(|error| anyhow::anyhow!("Codex model catalog output is invalid JSON: {error}"))?;
    let loaded_slugs = loaded_catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Codex model catalog output has no models array"))?
        .iter()
        .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
        .collect::<HashSet<_>>();
    let missing_slugs = expected_model_slugs
        .iter()
        .filter(|slug| !loaded_slugs.contains(slug.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_slugs.is_empty() {
        anyhow::bail!(
            "Codex did not load {} managed models: {}",
            missing_slugs.len(),
            missing_slugs.join(", ")
        );
    }
    Ok(())
}

fn resolve_codex_cli() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
        anyhow::bail!(
            "CODEX_CLI_PATH does not point to a file: {}",
            path.display()
        );
    }
    for path in [
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
    ] {
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join("codex");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    anyhow::bail!(
        "Codex CLI was not found; set CODEX_CLI_PATH or install Codex before installing Codex Mixin"
    )
}

fn managed_catalog_path(doc: &DocumentMut, config_path: &Path) -> anyhow::Result<PathBuf> {
    let catalog_path = PathBuf::from(
        doc.get("model_catalog_json")
            .and_then(Item::as_str)
            .ok_or_else(|| anyhow::anyhow!("managed Codex config has no model_catalog_json"))?,
    );
    if catalog_path.is_absolute() {
        absolute_path(catalog_path)
    } else {
        absolute_path(
            config_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?
                .join(catalog_path),
        )
    }
}

fn read_managed_config_for_install(config_path: &Path) -> anyhow::Result<String> {
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
            "existing codex-mixin restore point found but current config is not managed: {} or {}",
            backup_path.display(),
            absent_marker_path.display()
        );
    }
    Ok(raw_config)
}

fn create_managed_config_restore_point(config_path: &Path, raw_config: &str) -> anyhow::Result<()> {
    if is_managed_config(raw_config) {
        return Ok(());
    }
    let backup_path = managed_backup_path(config_path);
    let absent_marker_path = managed_absent_marker_path(config_path);
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if config_path.exists() {
        fs::copy(config_path, &backup_path)?;
    } else {
        fs::write(&absent_marker_path, b"")?;
    }
    Ok(())
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

fn codex_home_path() -> PathBuf {
    std::env::var("CODEX_HOME").ok().map_or_else(
        || {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".codex")
        },
        PathBuf::from,
    )
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
    default_model: Option<&str>,
    catalog_path: &std::path::Path,
    base_url: &str,
    web_search: &str,
    env_key: Option<&str>,
    codex_oauth_proxy: bool,
) -> anyhow::Result<()> {
    doc["model_catalog_json"] = value(catalog_path.to_string_lossy().to_string());
    doc["model_provider"] = value(CODEX_MIXIN_PROVIDER);
    doc["web_search"] = value(web_search);
    if let Some(model) = default_model {
        doc["model"] = value(model);
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
    let mut provider_table = Table::new();
    provider_table["name"] = value("Codex Mixin");
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
    providers.insert(CODEX_MIXIN_PROVIDER, Item::Table(provider_table));
    Ok(())
}

fn persist_gateway_bind(bind: SocketAddr) -> anyhow::Result<bool> {
    let Some(mut stored) = load_stored_config()? else {
        return Ok(false);
    };
    let bind = bind.to_string();
    if stored.gateway_bind.as_deref() == Some(&bind) {
        return Ok(false);
    }
    stored.gateway_bind = Some(bind);
    save_stored_config(&stored)?;
    Ok(true)
}

fn sync_managed_codex_gateway_base_url(
    config_path: &Path,
    bind: SocketAddr,
) -> anyhow::Result<bool> {
    let config_path = absolute_path(config_path.to_path_buf())?;
    if !config_path.exists() {
        return Ok(false);
    }
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
    let raw_config = fs::read_to_string(&config_path)?;
    if !is_managed_config(&raw_config) {
        return Ok(false);
    }
    let mut doc = raw_config.parse::<DocumentMut>()?;
    let provider = doc
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .and_then(|providers| providers.get_mut(CODEX_MIXIN_PROVIDER))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("managed Codex config has no codex-mixin provider"))?;
    let base_url = format!("http://{bind}/v1");
    if provider.get("base_url").and_then(Item::as_str) == Some(base_url.as_str()) {
        return Ok(false);
    }
    provider["base_url"] = value(base_url);
    write_atomic_if_changed(&config_path, doc.to_string().as_bytes())
}

async fn bind_gateway_listener(
    bind: SocketAddr,
    automatic_bind: bool,
) -> anyhow::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => Ok(listener),
        Err(err)
            if automatic_bind
                && bind.ip().is_loopback()
                && err.kind() == io::ErrorKind::AddrInUse =>
        {
            Ok(tokio::net::TcpListener::bind(SocketAddr::new(bind.ip(), 0)).await?)
        }
        Err(err) => Err(err.into()),
    }
}

async fn start(
    bind: Option<SocketAddr>,
    daemon: bool,
    log_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let mut config = GatewayConfig::from_env()?;
    let automatic_bind = bind.is_none()
        && std::env::var("CODEX_GATEWAY_BIND")
            .ok()
            .is_none_or(|value| value.is_empty());
    if let Some(bind) = bind {
        config.bind = bind;
    }
    if daemon {
        return start_daemon(bind, log_file);
    }
    if let Some(runtime) = load_runtime_metadata()? {
        if pid_is_running(runtime.pid)? {
            anyhow::bail!(
                "gateway already running: pid {}, bind {}",
                runtime.pid,
                runtime.bind
            );
        }
        tracing::warn!(pid = runtime.pid, "removing stale gateway runtime metadata");
        delete_runtime_metadata()?;
    }
    let listener = bind_gateway_listener(config.bind, automatic_bind).await?;
    let actual_bind = listener.local_addr()?;
    config.bind = actual_bind;
    if automatic_bind {
        persist_gateway_bind(actual_bind)?;
    }
    let config_path = resolve_codex_config_path(None)?;
    sync_managed_codex_gateway_base_url(&config_path, actual_bind)?;
    let supported_models = WebSearchCapabilities::from_default_path(&config)?.supported_model_ids();
    match refresh_managed_codex_catalog_with_capabilities(&config_path, Some(&supported_models)) {
        Ok(true) => tracing::info!("Codex model catalog refreshed"),
        Ok(false) => {}
        Err(err) => tracing::warn!(error = %err, "failed to refresh Codex model catalog"),
    }
    let refresh_config = config.clone();
    let refresh_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(CODEX_CATALOG_REFRESH_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            let refresh_result = WebSearchCapabilities::from_default_path(&refresh_config)
                .map(|capabilities| capabilities.supported_model_ids())
                .and_then(|supported_models| {
                    refresh_managed_codex_catalog_with_capabilities(
                        &config_path,
                        Some(&supported_models),
                    )
                });
            match refresh_result {
                Ok(true) => tracing::info!("Codex model catalog refreshed"),
                Ok(false) => {}
                Err(err) => tracing::warn!(error = %err, "failed to refresh Codex model catalog"),
            }
        }
    });
    let pid = std::process::id();
    save_runtime_metadata(&RuntimeMetadata {
        pid,
        bind: actual_bind,
        started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
    })?;
    let _runtime_guard = RuntimeMetadataGuard { pid };
    let result = serve_on_listener(config, listener).await;
    refresh_task.abort();
    match &result {
        Ok(()) => tracing::info!(pid, "gateway stopped"),
        Err(error) => tracing::error!(pid, error = %error, "gateway stopped with error"),
    }
    result
}

fn start_daemon(mut bind: Option<SocketAddr>, log_file: Option<PathBuf>) -> anyhow::Result<()> {
    let daemon = load_daemon_metadata()?;
    let runtime = load_runtime_metadata()?;
    let daemon_running = daemon
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    let runtime_running = runtime
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    if daemon_running
        && runtime_running
        && daemon.as_ref().map(|metadata| metadata.pid)
            != runtime.as_ref().map(|metadata| metadata.pid)
    {
        anyhow::bail!(
            "conflicting live gateway metadata: daemon pid {}, runtime pid {}",
            daemon.as_ref().expect("live daemon metadata").pid,
            runtime.as_ref().expect("live runtime metadata").pid
        );
    }
    if runtime_running {
        let runtime = runtime.as_ref().expect("live runtime metadata");
        if let Some(existing_bind) =
            replacement_bind_for_outdated_runtime(runtime, env!("CARGO_PKG_VERSION"))
        {
            println!(
                "replacing gateway version {} with {} on {}",
                runtime.version.as_deref().unwrap_or("unknown"),
                env!("CARGO_PKG_VERSION"),
                existing_bind
            );
            stop(false)?;
            if bind.is_none() {
                bind = Some(existing_bind);
            }
        } else if daemon_running {
            anyhow::bail!(
                "gateway daemon already running: pid {}, bind {}",
                runtime.pid,
                runtime.bind
            );
        } else {
            anyhow::bail!(
                "gateway already running: pid {}, bind {}",
                runtime.pid,
                runtime.bind
            );
        }
    } else if daemon_running {
        let daemon = daemon.as_ref().expect("live daemon metadata");
        println!(
            "replacing gateway with missing runtime metadata on {}",
            daemon.bind
        );
        let existing_bind = daemon.bind;
        stop(false)?;
        if bind.is_none() {
            bind = Some(existing_bind);
        }
    }
    delete_daemon_metadata()?;
    delete_runtime_metadata()?;
    let log_file = log_file.unwrap_or_else(default_log_file_path);
    if let Some(parent) = log_file.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)?;
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command.arg("start").arg("--log-file").arg(&log_file);
    if let Some(bind) = bind {
        command.arg("--bind").arg(bind.to_string());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn()?;
    let pid = child.id();
    let mut actual_bind = None;
    for _ in 0..50 {
        if !pid_is_running(pid)? {
            anyhow::bail!(
                "gateway daemon exited immediately; inspect log: {}",
                log_file.display()
            );
        }
        if let Some(runtime) = load_runtime_metadata()?
            && runtime.pid == pid
        {
            actual_bind = Some(runtime.bind);
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let bind = actual_bind.ok_or_else(|| {
        anyhow::anyhow!(
            "gateway daemon did not publish its endpoint within 5s; inspect log: {}",
            log_file.display()
        )
    })?;
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
    let daemon = load_daemon_metadata()?;
    let runtime = load_runtime_metadata()?;
    let daemon_running = daemon
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    let runtime_running = runtime
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    if daemon_running
        && runtime_running
        && daemon.as_ref().map(|metadata| metadata.pid)
            != runtime.as_ref().map(|metadata| metadata.pid)
    {
        anyhow::bail!(
            "conflicting live gateway metadata: daemon pid {}, runtime pid {}",
            daemon.as_ref().expect("live daemon metadata").pid,
            runtime.as_ref().expect("live runtime metadata").pid
        );
    }
    let (pid, process_kind) = if daemon_running {
        (daemon.as_ref().expect("live daemon metadata").pid, "daemon")
    } else if runtime_running {
        (
            runtime.as_ref().expect("live runtime metadata").pid,
            "foreground",
        )
    } else {
        let stale_pid = daemon
            .as_ref()
            .map(|metadata| metadata.pid)
            .or_else(|| runtime.as_ref().map(|metadata| metadata.pid));
        delete_daemon_metadata()?;
        delete_runtime_metadata()?;
        if let Some(pid) = stale_pid {
            println!("removed stale gateway metadata for pid {pid}");
        } else {
            println!("gateway is not recorded");
        }
        return Ok(());
    };
    send_signal(pid, "TERM")?;
    for _ in 0..50 {
        if !pid_is_running(pid)? {
            delete_daemon_metadata()?;
            delete_runtime_metadata()?;
            println!("gateway {process_kind} stopped: pid {pid}");
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    if force {
        send_signal(pid, "KILL")?;
        delete_daemon_metadata()?;
        delete_runtime_metadata()?;
        println!("gateway {process_kind} killed: pid {pid}");
        return Ok(());
    }
    anyhow::bail!(
        "gateway {process_kind} did not stop within 5s: pid {pid}. Use --force to send SIGKILL"
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

fn runtime_metadata_path() -> PathBuf {
    std::env::var("CODEX_GATEWAY_RUNTIME_FILE")
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir().join("runtime.json"))
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

fn load_runtime_metadata() -> anyhow::Result<Option<RuntimeMetadata>> {
    let path = runtime_metadata_path();
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

fn replacement_bind_for_outdated_runtime(
    runtime: &RuntimeMetadata,
    current_version: &str,
) -> Option<SocketAddr> {
    (runtime.version.as_deref() != Some(current_version)).then_some(runtime.bind)
}

fn save_daemon_metadata(metadata: &DaemonMetadata) -> anyhow::Result<()> {
    let path = daemon_metadata_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
}

fn save_runtime_metadata(metadata: &RuntimeMetadata) -> anyhow::Result<()> {
    let path = runtime_metadata_path();
    write_atomic_if_changed(&path, &serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
}

fn delete_daemon_metadata() -> anyhow::Result<()> {
    let path = daemon_metadata_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn delete_runtime_metadata() -> anyhow::Result<()> {
    let path = runtime_metadata_path();
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
    fn resolves_explicit_preserved_and_default_image_generation_paths() {
        assert_eq!(
            resolve_image_generation_path(
                Some(" /custom/images ".to_owned()),
                false,
                Some("/old/images".to_owned()),
                Some("/default/images"),
            ),
            Some("/custom/images".to_owned())
        );
        assert_eq!(
            resolve_image_generation_path(
                None,
                false,
                Some("/old/images".to_owned()),
                Some("/default/images"),
            ),
            Some("/old/images".to_owned())
        );
        assert_eq!(
            resolve_image_generation_path(None, true, None, Some("/default/images")),
            Some("/default/images".to_owned())
        );
        assert_eq!(
            resolve_image_generation_path(
                Some("  ".to_owned()),
                false,
                Some("/old/images".to_owned()),
                Some("/default/images"),
            ),
            Some(String::new())
        );
    }

    #[test]
    fn rotates_gateway_log_at_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gateway.log");
        fs::write(&log, b"12345").unwrap();

        rotate_gateway_log_if_needed(&log, 5).unwrap();

        assert!(!log.exists());
        assert_eq!(
            fs::read(dir.path().join("gateway.log.1")).unwrap(),
            b"12345"
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(dir.path().join("gateway.log.1"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn keeps_gateway_log_below_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("gateway.log");
        fs::write(&log, b"1234").unwrap();

        rotate_gateway_log_if_needed(&log, 5).unwrap();

        assert_eq!(fs::read(log).unwrap(), b"1234");
        assert!(!dir.path().join("gateway.log.1").exists());
    }

    #[test]
    fn install_command_rejects_provider_override() {
        assert!(
            Cli::try_parse_from(["codex-mixin", "install-codex", "--provider", "custom"]).is_err()
        );
    }

    #[test]
    fn oauth_proxy_install_supports_first_run_config_without_provider() {
        let mut doc = r#"
[projects."/Users/example/work"]
trust_level = "trusted"

[hooks.state]
"#
        .parse::<DocumentMut>()
        .unwrap();
        let catalog_path = PathBuf::from("/tmp/mixin-models.json");

        upsert_codex_config(
            &mut doc,
            None,
            &catalog_path,
            "http://127.0.0.1:8787/v1",
            "disabled",
            None,
            true,
        )
        .unwrap();

        assert_eq!(doc["model_provider"].as_str(), Some("codex-mixin"));
        assert_eq!(doc["web_search"].as_str(), Some("disabled"));
        assert_eq!(
            doc["model_providers"]["codex-mixin"]["base_url"].as_str(),
            Some("http://127.0.0.1:8787/v1")
        );
        assert_eq!(
            doc["projects"]["/Users/example/work"]["trust_level"].as_str(),
            Some("trusted")
        );
    }

    #[test]
    fn configures_web_search_without_changing_default_model() {
        let mut doc = DocumentMut::new();
        upsert_codex_config(
            &mut doc,
            None,
            Path::new("/tmp/mixin-models.json"),
            "http://127.0.0.1:8787/v1",
            "live",
            None,
            true,
        )
        .unwrap();

        assert_eq!(doc["web_search"].as_str(), Some("live"));
        assert!(doc.get("model").is_none());
    }

    #[test]
    fn custom_config_controls_default_catalog_and_models_cache_paths() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("managed-codex").join("config.toml");

        let paths = resolve_codex_install_paths(Some(config_path.clone()), None).unwrap();

        assert_eq!(paths.config, config_path);
        assert_eq!(
            paths.catalog,
            dir.path()
                .join("managed-codex")
                .join("model-catalogs")
                .join("mixin-models.json")
        );
        assert_eq!(
            paths.models_cache,
            dir.path().join("managed-codex").join("models_cache.json")
        );
    }

    #[test]
    fn explicit_relative_config_and_catalog_paths_become_absolute() {
        let relative_config = PathBuf::from("target/codex-mixin-test/config.toml");
        let relative_catalog = PathBuf::from("target/codex-mixin-test/catalog.json");

        let paths = resolve_codex_install_paths(
            Some(relative_config.clone()),
            Some(relative_catalog.clone()),
        )
        .unwrap();

        assert_eq!(paths.config, std::path::absolute(relative_config).unwrap());
        assert_eq!(
            paths.catalog,
            std::path::absolute(relative_catalog).unwrap()
        );
        assert!(paths.models_cache.is_absolute());
    }

    #[test]
    fn oauth_install_missing_cache_creates_no_restore_marker_or_directory() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("managed-codex").join("config.toml");
        let paths = resolve_codex_install_paths(Some(config_path.clone()), None).unwrap();

        let error = load_codex_install_template(&paths, true).unwrap_err();

        assert!(error.to_string().contains("model cache is missing"));
        assert!(!config_path.parent().unwrap().exists());
        assert!(!managed_backup_path(&config_path).exists());
        assert!(!managed_absent_marker_path(&config_path).exists());
    }

    #[test]
    fn managed_install_backup_and_uninstall_restore_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let catalog_path = dir.path().join("model-catalogs").join("mixin-models.json");
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        let original_config = "model_provider = \"custom\"\nmodel = \"gpt-5.5\"\n";
        fs::write(&config_path, original_config).unwrap();
        fs::write(&catalog_path, "{}").unwrap();
        let session_path = dir.path().join("sessions/legacy.jsonl");
        fs::create_dir_all(session_path.parent().unwrap()).unwrap();
        fs::write(
            &session_path,
            r#"{"type":"session_meta","payload":{"model_provider":"codex-mixin"}}"#,
        )
        .unwrap();

        let original = read_managed_config_for_install(&config_path).unwrap();
        assert_eq!(original, original_config);
        create_managed_config_restore_point(&config_path, &original).unwrap();
        assert!(managed_backup_path(&config_path).exists());
        fs::write(
            &config_path,
            format!(
                "{MANAGED_CONFIG_HEADER}\nmodel = \"Claude Sonnet 5\"\nmodel_catalog_json = {:?}\n",
                catalog_path.to_string_lossy()
            ),
        )
        .unwrap();

        uninstall_codex(Some(config_path.clone()), None).unwrap();
        assert_eq!(fs::read_to_string(&config_path).unwrap(), original_config);
        assert!(
            fs::read_to_string(&session_path)
                .unwrap()
                .contains(r#""model_provider":"custom""#)
        );
        assert!(!managed_backup_path(&config_path).exists());
        assert!(!catalog_path.exists());
    }

    #[test]
    fn failed_codex_validation_rolls_back_config_catalog_and_restore_point() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let catalog_path = dir.path().join("mixin-models.json");
        let original_config = b"model_provider = \"openai\"\n";
        let original_catalog = b"{\"models\":[{\"slug\":\"original\"}]}";
        fs::write(&config_path, original_config).unwrap();
        fs::write(&catalog_path, original_catalog).unwrap();
        let paths = CodexInstallPaths {
            config: config_path.clone(),
            catalog: catalog_path.clone(),
            models_cache: dir.path().join("models_cache.json"),
        };

        let error = write_managed_codex_files(
            &paths,
            std::str::from_utf8(original_config).unwrap(),
            b"{\"models\":[{\"slug\":\"custom\"}]}",
            format!("{MANAGED_CONFIG_HEADER}\nmodel_provider = \"codex-mixin\"\n").as_bytes(),
            || anyhow::bail!("validator rejected candidate"),
        )
        .unwrap_err();

        assert!(error.to_string().contains("installation rolled back"));
        assert_eq!(fs::read(&config_path).unwrap(), original_config);
        assert_eq!(fs::read(&catalog_path).unwrap(), original_catalog);
        assert!(!managed_backup_path(&config_path).exists());
        assert!(!managed_absent_marker_path(&config_path).exists());
    }

    #[test]
    fn managed_uninstall_removes_config_when_none_existed_before() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let catalog_path = dir.path().join("model-catalogs").join("mixin-models.json");

        let original = read_managed_config_for_install(&config_path).unwrap();
        assert!(original.is_empty());
        create_managed_config_restore_point(&config_path, &original).unwrap();
        assert!(managed_absent_marker_path(&config_path).exists());
        fs::write(
            &config_path,
            format!(
                "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n",
                catalog_path.to_string_lossy()
            ),
        )
        .unwrap();
        let session_path = dir.path().join("sessions/first-run.jsonl");
        fs::create_dir_all(session_path.parent().unwrap()).unwrap();
        fs::write(
            &session_path,
            r#"{"type":"session_meta","payload":{"model_provider":"codex-mixin"}}"#,
        )
        .unwrap();

        uninstall_codex(Some(config_path.clone()), Some(catalog_path)).unwrap();
        assert!(!config_path.exists());
        assert!(!managed_absent_marker_path(&config_path).exists());
        assert!(
            fs::read_to_string(session_path)
                .unwrap()
                .contains(r#""model_provider":"openai""#)
        );
    }

    #[test]
    fn oauth_proxy_install_replaces_legacy_custom_provider() {
        let mut doc = r#"
model_provider = "custom"
model = "gpt-5.5"

[model_providers.custom]
name = "OpenAI"
requires_openai_auth = true
supports_websockets = true
wire_api = "responses"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let catalog_path = PathBuf::from("/tmp/mixin-models.json");

        upsert_codex_config(
            &mut doc,
            None,
            &catalog_path,
            "http://127.0.0.1:8787/v1",
            "disabled",
            None,
            true,
        )
        .unwrap();

        assert_eq!(doc["model_provider"].as_str(), Some("codex-mixin"));
        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(
            doc["model_providers"]["codex-mixin"]["base_url"].as_str(),
            Some("http://127.0.0.1:8787/v1")
        );
        assert_eq!(
            doc["model_providers"]["codex-mixin"]["requires_openai_auth"].as_bool(),
            Some(true)
        );
        assert_eq!(
            doc["model_providers"]["custom"]["wire_api"].as_str(),
            Some("responses")
        );
    }

    #[test]
    fn oauth_proxy_install_replaces_conflicting_mixin_provider_table() {
        let mut doc = r#"
[model_providers.codex-mixin]
name = "stale"
base_url = "https://stale.example/v1"
env_key = "STALE_KEY"
experimental_bearer_token = "stale-token"
custom_field = "stale"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let catalog_path = PathBuf::from("/tmp/mixin-models.json");

        upsert_codex_config(
            &mut doc,
            None,
            &catalog_path,
            "http://127.0.0.1:8787/v1",
            "disabled",
            None,
            true,
        )
        .unwrap();

        let provider = doc["model_providers"]["codex-mixin"].as_table().unwrap();
        assert_eq!(provider["name"].as_str(), Some("Codex Mixin"));
        assert_eq!(
            provider["base_url"].as_str(),
            Some("http://127.0.0.1:8787/v1")
        );
        assert_eq!(provider["requires_openai_auth"].as_bool(), Some(true));
        assert!(provider.get("env_key").is_none());
        assert!(provider.get("experimental_bearer_token").is_none());
        assert!(provider.get("custom_field").is_none());
    }

    #[test]
    fn oauth_proxy_install_writes_codex_mixin_provider_without_default_model() {
        let mut doc = "model = \"gpt-5.5\"\n".parse::<DocumentMut>().unwrap();
        let catalog_path = PathBuf::from("/tmp/mixin-models.json");

        upsert_codex_config(
            &mut doc,
            None,
            &catalog_path,
            "http://127.0.0.1:8787/v1",
            "disabled",
            None,
            true,
        )
        .unwrap();

        assert_eq!(doc["model_provider"].as_str(), Some("codex-mixin"));
        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(
            doc["model_providers"]["codex-mixin"]["base_url"].as_str(),
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
                "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nrequires_openai_auth = true\n",
                catalog_path.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            &official_path,
            r#"{"models":[{"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol"},{"slug":"gpt-5.6-terra","display_name":"GPT-5.6-Terra"},{"slug":"gpt-5.6-luna","display_name":"GPT-5.6-Luna"}]}"#,
        )
        .unwrap();
        fs::write(
            &catalog_path,
            r#"{"models":[{"slug":"gpt-5.5","display_name":"GPT-5.5"},{"slug":"DeepSeek-V4-Flash","description":"Custom upstream model exposed through codex-mixin"}]}"#,
        )
        .unwrap();

        assert!(refresh_managed_codex_catalog(&config_path).unwrap());
        let refreshed: serde_json::Value =
            serde_json::from_slice(&fs::read(&catalog_path).unwrap()).unwrap();
        assert_eq!(refreshed["models"][0]["slug"], "gpt-5.6-sol");
        assert_eq!(refreshed["models"][1]["slug"], "gpt-5.6-terra");
        assert_eq!(refreshed["models"][2]["slug"], "gpt-5.6-luna");
        assert_eq!(refreshed["models"][3]["slug"], "DeepSeek-V4-Flash");
        assert_eq!(refreshed["models"][3]["multi_agent_version"], "v2");
        for model in refreshed["models"].as_array().unwrap() {
            assert!(model["base_instructions"].is_string());
            assert!(model["model_messages"]["instructions_template"].is_string());
        }
        assert!(!refresh_managed_codex_catalog(&config_path).unwrap());
    }

    #[test]
    fn non_oauth_managed_config_skips_oauth_catalog_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            format!(
                "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nwire_api = \"responses\"\n",
                dir.path().join("mixin-models.json").to_string_lossy()
            ),
        )
        .unwrap();

        assert!(!refresh_managed_codex_catalog(&config_path).unwrap());
    }

    #[test]
    fn refreshes_per_model_web_search_for_non_oauth_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let catalog_path = dir.path().join("mixin-models.json");
        fs::write(
            &config_path,
            format!(
                "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nwire_api = \"responses\"\n",
                catalog_path.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            &catalog_path,
            r#"{"models":[{"slug":"Claude Haiku 4.5","codex_mixin_managed":true},{"slug":"DeepSeek-V4-Flash","codex_mixin_managed":true,"web_search_tool_type":"text"}]}"#,
        )
        .unwrap();

        let supported_models = HashSet::from(["Claude Haiku 4.5".to_owned()]);
        assert!(
            refresh_managed_codex_catalog_with_capabilities(&config_path, Some(&supported_models))
                .unwrap()
        );
        let refreshed: serde_json::Value =
            serde_json::from_slice(&fs::read(catalog_path).unwrap()).unwrap();
        assert_eq!(refreshed["models"][0]["web_search_tool_type"], "text");
        assert!(refreshed["models"][1].get("web_search_tool_type").is_none());
    }

    #[test]
    fn uninstall_rejects_catalog_that_differs_from_managed_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let managed_catalog_path = dir.path().join("managed-models.json");
        let explicit_catalog_path = dir.path().join("other-models.json");
        fs::write(
            &config_path,
            format!(
                "{MANAGED_CONFIG_HEADER}\nmodel_catalog_json = {:?}\n",
                managed_catalog_path.to_string_lossy()
            ),
        )
        .unwrap();
        fs::write(
            managed_backup_path(&config_path),
            "model_provider = \"openai\"\n",
        )
        .unwrap();
        fs::write(&managed_catalog_path, "{}").unwrap();
        fs::write(&explicit_catalog_path, "{}").unwrap();

        let error = uninstall_codex(
            Some(config_path.clone()),
            Some(explicit_catalog_path.clone()),
        )
        .unwrap_err();

        assert!(error.to_string().contains("does not match"));
        assert!(is_managed_config(&fs::read_to_string(config_path).unwrap()));
        assert!(managed_catalog_path.exists());
        assert!(explicit_catalog_path.exists());
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

    #[tokio::test]
    async fn automatic_bind_uses_an_available_loopback_port() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_bind = occupied.local_addr().unwrap();

        let automatic = bind_gateway_listener(occupied_bind, true).await.unwrap();
        assert_ne!(automatic.local_addr().unwrap(), occupied_bind);

        let explicit = bind_gateway_listener(occupied_bind, false)
            .await
            .unwrap_err();
        assert_eq!(
            explicit.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::AddrInUse
        );
    }

    #[test]
    fn outdated_gateway_runtime_is_replaced_on_its_existing_bind() {
        let legacy_runtime: RuntimeMetadata =
            serde_json::from_str(r#"{"pid":42,"bind":"127.0.0.1:18787","started_at":1}"#).unwrap();
        let older_runtime: RuntimeMetadata = serde_json::from_str(
            r#"{"pid":42,"bind":"127.0.0.1:18787","started_at":1,"version":"0.2.15"}"#,
        )
        .unwrap();
        let current_runtime: RuntimeMetadata = serde_json::from_value(serde_json::json!({
            "pid": 42,
            "bind": "127.0.0.1:18787",
            "started_at": 1,
            "version": env!("CARGO_PKG_VERSION"),
        }))
        .unwrap();
        let existing_bind = "127.0.0.1:18787".parse().unwrap();

        assert_eq!(
            replacement_bind_for_outdated_runtime(&legacy_runtime, env!("CARGO_PKG_VERSION")),
            Some(existing_bind)
        );
        assert_eq!(
            replacement_bind_for_outdated_runtime(&older_runtime, env!("CARGO_PKG_VERSION")),
            Some(existing_bind)
        );
        assert_eq!(
            replacement_bind_for_outdated_runtime(&current_runtime, env!("CARGO_PKG_VERSION")),
            None
        );
    }

    #[test]
    fn syncs_dynamic_gateway_port_to_managed_codex_provider() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(
            &config_path,
            format!(
                "{MANAGED_CONFIG_HEADER}\n\n[model_providers.codex-mixin]\nbase_url = \"http://127.0.0.1:8787/v1\"\nwire_api = \"responses\"\n\n[model_providers.other]\nbase_url = \"https://example.test/v1\"\n"
            ),
        )
        .unwrap();

        assert!(
            sync_managed_codex_gateway_base_url(&config_path, "127.0.0.1:18787".parse().unwrap())
                .unwrap()
        );
        let doc = fs::read_to_string(&config_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            doc["model_providers"]["codex-mixin"]["base_url"].as_str(),
            Some("http://127.0.0.1:18787/v1")
        );
        assert_eq!(
            doc["model_providers"]["other"]["base_url"].as_str(),
            Some("https://example.test/v1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_rewrite_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        assert!(write_atomic_if_changed(&path, b"new").unwrap());

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
