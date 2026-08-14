use std::fs;
use std::io::Write;
use std::io::{self, IsTerminal};
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use codex_mixin::catalog::{codex_catalog_from_models_with_metadata, load_template_catalog};
use codex_mixin::config::{GatewayConfig, load_stored_config};
use codex_mixin::provider::ProviderPreset;
use codex_mixin::server::AppState;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

mod atomic_file;
mod benchmark_proxy;
mod claude;
mod codex;
mod config_input;
mod doctor;
mod dsh;
mod ducx_setup;
mod fusion_config;
mod maintenance;
mod metadata;
mod providers;
mod report_hook;
mod runtime;
mod service;
mod status;

use benchmark_proxy::{benchmark_start, benchmark_status};
use claude::{claude_status, install_claude, sync_claude_hooks, uninstall_claude};
use codex::{
    InstallCodexOptions, install_codex, refresh_default_managed_codex_catalog,
    resolve_codex_install_paths, uninstall_codex,
};
use doctor::doctor;
use dsh::{install_dsh, uninstall_dsh};
use ducx_setup::ensure_managed_ducx;
use fusion_config::{delete_fusion_profile, get_fusion_profile, set_fusion_profile};
use maintenance::migrate_history;
use metadata::{load_model_metadata_resolver, refresh_metadata};
use providers::{
    AddProviderOptions, UpdateProviderOptions, add_provider, discover_models,
    discover_models_with_output, list_providers, remove_provider, select_models,
    set_provider_enabled, test_provider, update_provider,
};
use service::{init_tracing, logs, restart, start, stop};
use status::{models, probe_web_search, quota, show_config, status, usage};

fn progress_is_interactive() -> bool {
    io::stdout().is_terminal()
}

pub(super) fn progress_step(message: &str) {
    // macOS App streaming collector only watches stderr lines with this prefix.
    eprintln!("MIXIN_PROGRESS {message}");
}

pub(super) fn next_step_line(message: &str) {
    if progress_is_interactive() {
        println!("{} {message}", style("→").cyan().bold());
    } else {
        println!("next: {message}");
    }
}

pub(super) async fn stage<T>(
    label: &str,
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let interactive = progress_is_interactive();
    let started = Instant::now();
    let spinner = if interactive {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .expect("spinner template is valid")
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", ""]),
        );
        bar.set_message(label.to_owned());
        bar.enable_steady_tick(std::time::Duration::from_millis(80));
        Some(bar)
    } else {
        println!("{label} ...");
        None
    };
    let result = future.await;
    if let Some(bar) = spinner {
        bar.finish_and_clear();
    }
    match &result {
        Ok(_) if interactive => {
            println!(
                "{} {} ({:.1}s)",
                style("✓").green().bold(),
                label,
                started.elapsed().as_secs_f32()
            );
        }
        Ok(_) => {
            println!("ok: {label} ({:.1}s)", started.elapsed().as_secs_f32());
        }
        Err(_) if interactive => {
            println!("{} {}", style("✗").red().bold(), label);
        }
        Err(_) => {
            println!("failed: {label}");
        }
    }
    result
}

fn install_cli_command() -> anyhow::Result<()> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(());
    };
    let bin = PathBuf::from(home).join(".local/bin");
    fs::create_dir_all(&bin)?;
    let target = bin.join("codex-mixin");
    let source = std::env::current_exe()?;
    if source != target {
        fs::copy(source, &target)?;
    }
    println!("CLI command installed: {}", target.display());
    println!(
        "Add {} to PATH if `codex-mixin` is not found.",
        bin.display()
    );
    Ok(())
}

fn cli_release_target() -> anyhow::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => anyhow::bail!("automatic update is not available for {os}/{arch}"),
    }
}

fn release_version_from_redirect(effective_url: &str) -> anyhow::Result<String> {
    let segment = effective_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty() && *segment != "latest")
        .ok_or_else(|| {
            anyhow::anyhow!("GitHub did not redirect to a release; proxy or rate limit response")
        })?;
    let version = segment.trim_start_matches('v');
    ensure_version_chars(version)?;
    Ok(version.to_owned())
}

fn ensure_version_chars(version: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !version.is_empty()
            && version
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '.'
                    || character == '-'),
        "GitHub returned an invalid release version: {version}"
    );
    Ok(())
}

fn replace_executable(target: &Path, downloaded: &Path) -> anyhow::Result<()> {
    let parent = target
        .parent()
        .context("current executable has no parent directory")?;
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    let staged = parent.join(format!(".codex-mixin-update-{token}.new"));
    let backup = parent.join(format!(".codex-mixin-update-{token}.old"));
    fs::copy(downloaded, &staged)?;
    #[cfg(unix)]
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    let replace_result: anyhow::Result<()> = (|| -> anyhow::Result<()> {
        fs::rename(target, &backup).with_context(|| {
            format!("failed to move current executable to {}", backup.display())
        })?;
        if let Err(error) = fs::rename(&staged, target) {
            fs::rename(&backup, target).with_context(|| {
                format!(
                    "failed to restore {} after update failure: {error}",
                    target.display()
                )
            })?;
            return Err(error.into());
        }
        let _ = fs::remove_file(&backup);
        Ok(())
    })();
    if replace_result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    replace_result
}

async fn update_cli() -> anyhow::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let response = ProcessCommand::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
            "/dev/null",
            "--write-out",
            "%{url_effective}",
            "--max-time",
            "60",
            "https://github.com/Edward-lyz/codex-mixin/releases/latest",
        ])
        .output()
        .context("cannot query GitHub releases; check HTTP_PROXY/HTTPS_PROXY")?;
    anyhow::ensure!(
        response.status.success(),
        "cannot query GitHub releases; curl exited with {}",
        response.status
    );
    let effective_url = String::from_utf8(response.stdout)?.trim().to_owned();
    let latest = release_version_from_redirect(&effective_url)?;
    if latest == current {
        println!("codex-mixin {current} is already up to date.");
        return Ok(());
    }
    let asset = cli_release_target()?;
    let url = format!(
        "https://github.com/Edward-lyz/codex-mixin/releases/download/v{latest}/codex-mixin-cli-{asset}.tar.gz"
    );
    let temp = tempfile::tempdir()?;
    let archive = temp.path().join("codex-mixin.tar.gz");
    println!("Downloading codex-mixin {latest}...");
    let status = ProcessCommand::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--max-time",
            "600",
            "--output",
        ])
        .arg(&archive)
        .arg(&url)
        .status()?;
    anyhow::ensure!(
        status.success(),
        "download failed for {url}; download the asset manually"
    );
    let status = ProcessCommand::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(temp.path())
        .arg("--strip-components=1")
        .status()?;
    anyhow::ensure!(status.success(), "failed to unpack downloaded release");
    let downloaded = temp.path().join("codex-mixin");
    anyhow::ensure!(
        downloaded.is_file(),
        "release archive did not contain codex-mixin"
    );
    anyhow::ensure!(
        fs::metadata(&downloaded)?.len() > 1024 * 1024,
        "downloaded release is unexpectedly small: {}",
        downloaded.display()
    );
    replace_executable(&std::env::current_exe()?, &downloaded)?;
    println!("Updated codex-mixin to {latest}; restarting gateway...");
    restart(None, None, false).await
}

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Connect custom model providers to Codex, Claude, and DSH"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Internal: forward a Codex hook event to the managed DUCX data-report.
    #[command(hide = true)]
    ReportHook {
        #[arg(long)]
        event: String,
    },
    /// Add a provider, start the gateway, and print the next step.
    Setup {
        #[arg(
            long,
            value_enum,
            help = "Provider preset; omit in a TTY to choose interactively"
        )]
        preset: Option<CliProviderPreset>,
        #[arg(long, help = "API key; omit for an interactive prompt")]
        key: Option<String>,
        #[arg(
            long,
            help = "Baidu OneAPI quota username; omit for an interactive prompt"
        )]
        quota_username: Option<String>,
        #[arg(
            long,
            value_enum,
            help = "Codex integration mode; omit for an interactive choice"
        )]
        codex_mode: Option<SetupCodexMode>,
        #[arg(long, help = "Configure the provider without starting the gateway")]
        no_start: bool,
    },
    /// Update this CLI from the latest GitHub release and restart the gateway.
    Update,
    /// Configure model providers and select their models.
    #[command(name = "provider", visible_alias = "providers")]
    Providers {
        #[command(subcommand)]
        command: Box<ProviderCommand>,
    },
    /// Start, stop, inspect, and follow the local gateway.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Install Codex or Claude integration.
    Connect {
        #[command(subcommand)]
        command: ConnectCommand,
    },
    /// Show the current setup and gateway state.
    Info {
        #[arg(long)]
        json: bool,
    },
    #[command(hide = true)]
    Fusion {
        #[command(subcommand)]
        command: FusionCommand,
    },
    #[command(hide = true)]
    Benchmark {
        #[command(subcommand)]
        command: BenchmarkCommand,
    },
    /// Diagnose the current setup and optionally repair it.
    #[command(visible_alias = "check")]
    Doctor {
        #[arg(long)]
        json: bool,
        #[arg(
            long,
            help = "Automatically repair safe issues (permissions, stale state, gateway start, base_url, model catalog)"
        )]
        fix: bool,
        #[arg(
            long = "restart-apps",
            requires = "fix",
            help = "Allow --fix to restart the ChatGPT/Codex apps (interrupts live sessions)"
        )]
        restart_apps: bool,
        #[arg(
            long,
            help = "Use cache-only checks and skip the Codex engine probe; plain doctor stays deep"
        )]
        quick: bool,
    },
    #[command(hide = true)]
    Status {
        #[arg(long)]
        json: bool,
    },
    #[command(hide = true)]
    Models {
        #[arg(long)]
        json: bool,
    },
    #[command(hide = true)]
    Quota {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        provider: Option<String>,
    },
    #[command(hide = true)]
    Usage {
        #[arg(long)]
        json: bool,
    },
    #[command(hide = true)]
    Config {
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum, default_value_t = ConfigScope::Effective)]
        scope: ConfigScope,
    },
    #[command(hide = true)]
    Start {
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[arg(long)]
        daemon: bool,
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    #[command(hide = true)]
    Stop {
        #[arg(long)]
        force: bool,
    },
    #[command(hide = true)]
    Restart {
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    #[command(hide = true)]
    Logs {
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: usize,
        #[arg(short, long)]
        follow: bool,
    },
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        bind: Option<SocketAddr>,
    },
    #[command(hide = true)]
    Catalog {
        #[arg(long)]
        template_catalog: Option<PathBuf>,
    },
    #[command(name = "refresh-metadata")]
    #[command(hide = true)]
    RefreshMetadata {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    #[command(name = "migrate-history")]
    #[command(hide = true)]
    MigrateHistory {
        #[arg(long)]
        codex_home: Option<PathBuf>,
    },
    #[command(name = "install-codex", visible_alias = "codex-config", hide = true)]
    InstallCodex {
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        set_default: bool,
        #[arg(
            long,
            required_unless_present = "custom_only",
            conflicts_with = "custom_only",
            help = "Merge official GPT and custom models using Codex OpenAI auth; requires models_cache.json"
        )]
        codex_oauth_proxy: bool,
        #[arg(
            long,
            required_unless_present = "codex_oauth_proxy",
            conflicts_with = "codex_oauth_proxy",
            help = "Install only custom upstream models using a managed local login placeholder; official plugins and cloud features are unavailable"
        )]
        custom_only: bool,
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
    #[command(name = "uninstall-codex", hide = true)]
    UninstallCodex {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        catalog: Option<PathBuf>,
    },
    #[command(name = "install-claude", hide = true)]
    InstallClaude {
        #[arg(long)]
        settings: Option<PathBuf>,
        #[arg(long)]
        model: Option<String>,
    },
    #[command(name = "uninstall-claude", hide = true)]
    UninstallClaude {
        #[arg(long)]
        settings: Option<PathBuf>,
    },
    #[command(name = "claude-status", hide = true)]
    ClaudeStatus {
        #[arg(long)]
        settings: Option<PathBuf>,
    },
    #[command(name = "refresh-codex-catalog", hide = true)]
    RefreshCodexCatalog,
    #[command(name = "probe-web-search")]
    #[command(hide = true)]
    ProbeWebSearch {
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Start the gateway in the background by default.
    Start {
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[arg(long)]
        foreground: bool,
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    /// Stop the background gateway.
    Stop {
        #[arg(long)]
        force: bool,
    },
    /// Restart the background gateway.
    Restart {
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    /// Print or follow gateway logs.
    Logs {
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: usize,
        #[arg(short, long)]
        follow: bool,
    },
    /// Show gateway and provider status.
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ConnectCommand {
    /// Install Codex integration.
    Codex(InstallCodexOptions),
    /// Install and sign in to the managed DUCX authentication carrier.
    Ducx,
    /// Install Claude Code integration.
    Claude {
        #[arg(long)]
        settings_path: Option<PathBuf>,
        #[arg(long)]
        model: Option<String>,
    },
    /// Install the Codex Mixin gateway as a DeepSeek Harness provider.
    Dsh {
        #[arg(long)]
        dsh_home: Option<PathBuf>,
    },
    /// Show Claude Code integration status.
    Status {
        #[arg(long)]
        settings_path: Option<PathBuf>,
    },
    /// Remove Codex, Claude, or DSH integration.
    Remove {
        #[arg(value_parser = ["codex", "claude", "dsh"])]
        target: String,
        #[arg(long)]
        settings_path: Option<PathBuf>,
        #[arg(long)]
        dsh_home: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum FusionCommand {
    Get {
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Set {
        #[arg(long)]
        profile_json: String,
        #[arg(long)]
        replace_id: Option<String>,
    },
    Delete {
        #[arg(long)]
        id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum BenchmarkCommand {
    Status,
    Start {
        #[arg(long)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = codex_mixin::benchmark::BENCHMARK_TARGET_OUTPUT_TOKENS)]
        target_output_tokens: u64,
        #[arg(long = "provider")]
        providers: Vec<String>,
        #[arg(long = "model")]
        models: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ProviderCommand {
    /// List configured providers.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Add a provider from a preset.
    Add {
        #[arg(long, value_enum)]
        preset: CliProviderPreset,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        key: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        website_url: Option<String>,
        #[arg(long)]
        protocol: Option<String>,
        #[arg(long)]
        api_path: Option<String>,
        #[arg(long)]
        models_path: Option<String>,
        #[arg(long)]
        image_generation_path: Option<String>,
        #[arg(long)]
        quota_url: Option<String>,
        #[arg(long, help = "Quota username; required by the baidu-oneapi preset")]
        quota_username: Option<String>,
        #[arg(long, help = "OpenCode Go dashboard workspace ID")]
        quota_workspace_id: Option<String>,
        #[arg(long, help = "OpenCode Go dashboard auth cookie")]
        quota_auth_cookie: Option<String>,
        #[arg(long)]
        quota_currency: Option<String>,
        #[arg(long)]
        quota_parser: Option<String>,
        #[arg(long)]
        gateway_key: Option<String>,
        #[arg(long = "model")]
        static_models: Vec<String>,
        #[arg(long = "header-env", value_name = "NAME=ENV_VAR")]
        header_env: Vec<String>,
        #[arg(long, value_name = "disabled|ducx_loopback")]
        baidu_auth_bridge: Option<String>,
        #[arg(long, value_name = "PATH")]
        ducx_executable: Option<PathBuf>,
        #[arg(long, value_name = "BOOL")]
        baidu_code_report: Option<bool>,
    },
    /// Update an existing provider.
    Update {
        id: String,
        #[arg(long, value_name = "BOOL")]
        auxiliary_model_upstream: Option<bool>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, conflicts_with = "key")]
        clear_key: bool,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        website_url: Option<String>,
        #[arg(long)]
        protocol: Option<String>,
        #[arg(long)]
        api_path: Option<String>,
        #[arg(long)]
        models_path: Option<String>,
        #[arg(long)]
        image_generation_path: Option<String>,
        #[arg(long)]
        clear_image_generation: bool,
        #[arg(long)]
        quota_url: Option<String>,
        #[arg(long)]
        clear_quota: bool,
        #[arg(
            long,
            help = "Quota username; required by the Baidu OneAPI quota parser"
        )]
        quota_username: Option<String>,
        #[arg(long, conflicts_with = "clear_quota_workspace_id")]
        quota_workspace_id: Option<String>,
        #[arg(long, conflicts_with = "quota_workspace_id")]
        clear_quota_workspace_id: bool,
        #[arg(long, conflicts_with = "clear_quota_auth_cookie")]
        quota_auth_cookie: Option<String>,
        #[arg(long, conflicts_with = "quota_auth_cookie")]
        clear_quota_auth_cookie: bool,
        #[arg(long)]
        quota_currency: Option<String>,
        #[arg(long)]
        quota_parser: Option<String>,
        #[arg(long = "header-env", value_name = "NAME=ENV_VAR")]
        header_env: Vec<String>,
        #[arg(long, conflicts_with = "header_env")]
        clear_header_env: bool,
        #[arg(long, value_name = "disabled|ducx_loopback")]
        baidu_auth_bridge: Option<String>,
        #[arg(long, value_name = "PATH")]
        ducx_executable: Option<PathBuf>,
        #[arg(long, value_name = "BOOL")]
        baidu_code_report: Option<bool>,
    },
    /// Enable a provider.
    Enable { id: String },
    /// Disable a provider.
    Disable { id: String },
    /// Remove a provider.
    Remove { id: String },
    /// Refresh the provider model catalog.
    Discover { id: String },
    /// Test provider authentication and model access.
    Test {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Select models exposed by a provider.
    Select {
        id: String,
        #[arg(long = "model")]
        models: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ConfigScope {
    Stored,
    Effective,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum SetupCodexMode {
    Official,
    Custom,
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CliProviderPreset {
    #[value(name = "custom")]
    Custom,
    #[value(name = "baidu-oneapi")]
    BaiduOneApi,
    #[value(name = "openrouter")]
    OpenRouter,
    #[value(name = "deepseek")]
    DeepSeek,
    #[value(name = "opencode-go", alias = "opencode_go")]
    OpenCodeGo,
}

impl CliProviderPreset {
    fn as_provider_preset(self) -> ProviderPreset {
        match self {
            Self::Custom => ProviderPreset::Custom,
            Self::BaiduOneApi => ProviderPreset::BaiduOneApi,
            Self::OpenRouter => ProviderPreset::OpenRouter,
            Self::DeepSeek => ProviderPreset::DeepSeek,
            Self::OpenCodeGo => ProviderPreset::OpenCodeGo,
        }
    }

    fn as_str(self) -> &'static str {
        self.as_provider_preset().as_str()
    }
}

impl From<ProviderPreset> for CliProviderPreset {
    fn from(value: ProviderPreset) -> Self {
        match value {
            ProviderPreset::Custom => Self::Custom,
            ProviderPreset::BaiduOneApi => Self::BaiduOneApi,
            ProviderPreset::OpenRouter => Self::OpenRouter,
            ProviderPreset::DeepSeek => Self::DeepSeek,
            ProviderPreset::OpenCodeGo => Self::OpenCodeGo,
        }
    }
}

pub(crate) async fn entrypoint() {
    let cli = Cli::parse();
    let foreground_log_file = match &cli.command {
        Some(Command::Start {
            daemon: false,
            log_file: Some(path),
            ..
        }) => Some(path.clone()),
        Some(Command::Service {
            command:
                ServiceCommand::Start {
                    foreground: true,
                    log_file: Some(path),
                    ..
                },
        }) => Some(path.clone()),
        Some(Command::ReportHook { .. }) => Some(runtime::default_report_hook_log_path()),
        _ => None,
    };
    let quiet_parent_logs = foreground_log_file.is_none()
        && !matches!(
            &cli.command,
            Some(
                Command::Start { daemon: false, .. }
                    | Command::Serve { .. }
                    | Command::Service {
                        command: ServiceCommand::Start {
                            foreground: true,
                            ..
                        }
                    }
            )
        );
    if let Err(error) = init_tracing(foreground_log_file.as_deref(), quiet_parent_logs) {
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

fn read_secret(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    #[cfg(unix)]
    if !ProcessCommand::new("stty").arg("-echo").status()?.success() {
        anyhow::bail!("failed to disable terminal echo for secret input")
    }
    let mut value = String::new();
    let read_result = io::stdin().read_line(&mut value);
    #[cfg(unix)]
    if !ProcessCommand::new("stty").arg("echo").status()?.success() {
        anyhow::bail!("failed to restore terminal echo after secret input")
    }
    println!();
    read_result?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        anyhow::bail!("API key cannot be empty")
    }
    Ok(value)
}

fn choose_setup_codex_mode(mode: Option<SetupCodexMode>) -> anyhow::Result<SetupCodexMode> {
    if let Some(mode) = mode {
        return Ok(mode);
    }
    if !io::stdin().is_terminal() {
        return Ok(SetupCodexMode::Skip);
    }
    println!("\nConnect Codex:");
    println!("  1. Official account mode - keep ChatGPT login, plugins, and cloud features");
    println!("  2. Custom models only - no official account required");
    println!("  3. Skip for now");
    print!("Choose [1-3]: ");
    io::stdout().flush()?;
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    match choice.trim() {
        "1" => Ok(SetupCodexMode::Official),
        "2" => Ok(SetupCodexMode::Custom),
        "3" => Ok(SetupCodexMode::Skip),
        _ => anyhow::bail!("invalid Codex mode; choose 1, 2, or 3"),
    }
}

fn choose_setup_preset(preset: Option<CliProviderPreset>) -> anyhow::Result<CliProviderPreset> {
    if let Some(preset) = preset {
        return Ok(preset);
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "provider preset is required in non-interactive mode; pass --preset <preset>\navailable presets: {}\nexample: codex-mixin setup --preset baidu-oneapi",
            ProviderPreset::available_presets_csv()
        );
    }
    println!("Choose a provider preset:");
    for (index, preset) in ProviderPreset::ALL.iter().enumerate() {
        println!(
            "  {}. {} - {}",
            index + 1,
            preset.as_str(),
            preset.description()
        );
    }
    print!("Choose [1-{}]: ", ProviderPreset::ALL.len());
    io::stdout().flush()?;
    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let index = choice
        .trim()
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("invalid preset choice; enter a number"))?;
    ProviderPreset::ALL
        .get(index.saturating_sub(1))
        .copied()
        .filter(|_| index >= 1)
        .map(CliProviderPreset::from)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "invalid preset choice; choose a number between 1 and {}",
                ProviderPreset::ALL.len()
            )
        })
}

async fn setup(
    preset: Option<CliProviderPreset>,
    key: Option<String>,
    quota_username: Option<String>,
    codex_mode: Option<SetupCodexMode>,
    no_start: bool,
) -> anyhow::Result<()> {
    let preset = choose_setup_preset(preset)?.as_str();
    if no_start
        && matches!(
            codex_mode,
            Some(SetupCodexMode::Official | SetupCodexMode::Custom)
        )
    {
        anyhow::bail!("--no-start cannot be combined with Codex installation");
    }
    install_cli_command()?;
    let key = match key.or_else(|| std::env::var("CODEX_MIXIN_API_KEY").ok()) {
        Some(key) if !key.trim().is_empty() => key,
        _ if io::stdin().is_terminal() => read_secret(&format!("API key for {preset}: "))?,
        _ => anyhow::bail!(
            "API key is required; pass --key or set CODEX_MIXIN_API_KEY in non-interactive mode"
        ),
    };

    let quota_username = if preset == "baidu-oneapi" {
        match quota_username.or_else(|| std::env::var("CODEX_MIXIN_QUOTA_USERNAME").ok()) {
            Some(username) if !username.trim().is_empty() => Some(username),
            _ if io::stdin().is_terminal() => {
                print!("Baidu OneAPI quota username: ");
                io::stdout().flush()?;
                let mut username = String::new();
                io::stdin().read_line(&mut username)?;
                let username = username.trim().to_owned();
                if username.is_empty() {
                    anyhow::bail!("Baidu OneAPI quota username cannot be empty")
                }
                Some(username)
            }
            _ => anyhow::bail!(
                "Baidu OneAPI quota username is required; pass --quota-username or set CODEX_MIXIN_QUOTA_USERNAME in non-interactive mode"
            ),
        }
    } else {
        quota_username
    };

    let ducx_executable = if preset == "baidu-oneapi" {
        Some(
            stage(
                "Preparing managed DUCX authentication",
                ensure_managed_ducx(),
            )
            .await?,
        )
    } else {
        None
    };
    let existing_provider = load_stored_config()?.and_then(|config| {
        config
            .providers
            .into_iter()
            .find(|provider| provider.id == preset)
    });
    if let Some(existing_provider) = existing_provider {
        println!("Updating provider configuration: {preset}");
        if existing_provider.preset_id.as_deref() != Some(preset) {
            anyhow::bail!(
                "provider {preset} already exists with preset {}; choose another provider id",
                existing_provider.preset_id.as_deref().unwrap_or("custom")
            )
        }
        update_provider(UpdateProviderOptions {
            id: preset.to_owned(),
            key: Some(key),
            quota_username,
            baidu_auth_bridge: (preset == "baidu-oneapi").then(|| "ducx_loopback".to_owned()),
            ducx_executable,
            ..UpdateProviderOptions::default()
        })
        .await?;
    } else {
        println!("Adding provider configuration: {preset}");
        add_provider(AddProviderOptions {
            preset: preset.to_owned(),
            id: None,
            key,
            display_name: None,
            base_url: None,
            website_url: None,
            protocol: None,
            api_path: None,
            models_path: None,
            image_generation_path: None,
            quota_url: None,
            quota_username,
            quota_workspace_id: None,
            quota_auth_cookie: None,
            quota_currency: None,
            quota_parser: None,
            gateway_key: None,
            static_models: Vec::new(),
            header_env: Vec::new(),
            baidu_auth_bridge: (preset == "baidu-oneapi").then(|| "ducx_loopback".to_owned()),
            ducx_executable,
            baidu_code_report: None,
        })
        .await?;
    }
    stage(
        &format!("Refreshing provider models for {preset}"),
        discover_models_with_output(preset, true),
    )
    .await?;
    println!("Provider models refreshed.");

    let executable = std::env::current_exe()?.display().to_string();
    if no_start {
        println!("Provider configured. Next: {executable} service start");
        return Ok(());
    }

    stage(
        "Restarting gateway with the new provider configuration",
        restart(None, None, true),
    )
    .await?;
    println!("Gateway is ready.");
    let codex_mode = choose_setup_codex_mode(codex_mode)?;
    match codex_mode {
        SetupCodexMode::Official => {
            let paths = resolve_codex_install_paths(None, None)?;
            if !paths.models_cache.exists() {
                anyhow::bail!(
                    "official Codex catalog is missing at {}; sign in and open Codex once, then run `{executable} connect codex --codex-oauth-proxy`",
                    paths.models_cache.display()
                )
            }
            install_codex(InstallCodexOptions {
                requested_model: None,
                set_default: false,
                codex_oauth_proxy: true,
                custom_only: false,
                config_path: None,
                catalog_path: None,
                base_url: None,
                web_search: "live".to_owned(),
                env_key: None,
                no_env_key: false,
            })
            .await?;
        }
        SetupCodexMode::Custom => {
            install_codex(InstallCodexOptions {
                requested_model: None,
                set_default: true,
                codex_oauth_proxy: false,
                custom_only: true,
                config_path: None,
                catalog_path: None,
                base_url: None,
                web_search: "live".to_owned(),
                env_key: None,
                no_env_key: false,
            })
            .await?;
        }
        SetupCodexMode::Skip => {
            println!("Gateway started.");
            next_step_line(&format!(
                "Connect Codex later: {executable} connect codex --codex-oauth-proxy"
            ));
        }
    }
    println!();
    if progress_is_interactive() {
        println!("{}", style("Setup complete").green().bold());
    } else {
        println!("Setup complete.");
    }
    next_step_line("Restart the Codex app, or start a new Codex CLI session");
    next_step_line(&format!("Check status: {executable} info"));
    next_step_line(&format!("Diagnose issues: {executable} doctor"));
    Ok(())
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command.unwrap_or(Command::Info { json: false }) {
        Command::ReportHook { event } => report_hook::run(&event).await,
        Command::Setup {
            preset,
            key,
            quota_username,
            codex_mode,
            no_start,
        } => setup(preset, key, quota_username, codex_mode, no_start).await,
        Command::Update => update_cli().await,
        Command::Providers { command } => match *command {
            ProviderCommand::List { json } => list_providers(json),
            ProviderCommand::Add {
                preset,
                id,
                key,
                display_name,
                base_url,
                website_url,
                protocol,
                api_path,
                models_path,
                image_generation_path,
                quota_url,
                quota_username,
                quota_workspace_id,
                quota_auth_cookie,
                quota_currency,
                quota_parser,
                gateway_key,
                static_models,
                header_env,
                baidu_auth_bridge,
                ducx_executable,
                baidu_code_report,
            } => {
                // Auto-provision the managed DUCX install when the DUCX bridge is
                // selected without an explicit executable.
                let ducx_executable = match (baidu_auth_bridge.as_deref(), &ducx_executable) {
                    (Some("ducx_loopback"), None) => Some(ensure_managed_ducx().await?),
                    _ => ducx_executable,
                };
                add_provider(AddProviderOptions {
                    preset: preset.as_str().to_owned(),
                    id,
                    key,
                    display_name,
                    base_url,
                    website_url,
                    protocol,
                    api_path,
                    models_path,
                    image_generation_path,
                    quota_url,
                    quota_username,
                    quota_workspace_id,
                    quota_auth_cookie,
                    quota_currency,
                    quota_parser,
                    gateway_key,
                    static_models,
                    header_env,
                    baidu_auth_bridge,
                    ducx_executable,
                    baidu_code_report,
                })
                .await?;
                report_hook::sync_installation()
            }
            ProviderCommand::Update {
                id,
                auxiliary_model_upstream,
                key,
                clear_key,
                display_name,
                base_url,
                website_url,
                protocol,
                api_path,
                models_path,
                image_generation_path,
                clear_image_generation,
                quota_url,
                clear_quota,
                quota_username,
                quota_workspace_id,
                clear_quota_workspace_id,
                quota_auth_cookie,
                clear_quota_auth_cookie,
                quota_currency,
                quota_parser,
                header_env,
                clear_header_env,
                baidu_auth_bridge,
                ducx_executable,
                baidu_code_report,
            } => {
                let ducx_executable = match (baidu_auth_bridge.as_deref(), &ducx_executable) {
                    (Some("ducx_loopback"), None) => Some(ensure_managed_ducx().await?),
                    _ => ducx_executable,
                };
                update_provider(UpdateProviderOptions {
                    id,
                    auxiliary_model_upstream,
                    key,
                    clear_key,
                    display_name,
                    base_url,
                    website_url,
                    protocol,
                    api_path,
                    models_path,
                    image_generation_path,
                    clear_image_generation,
                    quota_url,
                    clear_quota,
                    quota_username,
                    quota_workspace_id,
                    clear_quota_workspace_id,
                    quota_auth_cookie,
                    clear_quota_auth_cookie,
                    quota_currency,
                    quota_parser,
                    header_env,
                    clear_header_env,
                    baidu_auth_bridge,
                    ducx_executable,
                    baidu_code_report,
                })
                .await?;
                report_hook::sync_installation()
            }
            ProviderCommand::Enable { id } => {
                set_provider_enabled(&id, true)?;
                report_hook::sync_installation()
            }
            ProviderCommand::Disable { id } => {
                set_provider_enabled(&id, false)?;
                report_hook::sync_installation()
            }
            ProviderCommand::Remove { id } => {
                remove_provider(&id)?;
                report_hook::sync_installation()
            }
            ProviderCommand::Discover { id } => discover_models(&id).await,
            ProviderCommand::Test { id, json } => test_provider(&id, json).await,
            ProviderCommand::Select { id, models } => select_models(&id, models),
        },
        Command::Service { command } => match command {
            ServiceCommand::Start {
                bind,
                foreground,
                log_file,
            } => start(bind, !foreground, log_file).await,
            ServiceCommand::Stop { force } => stop(force),
            ServiceCommand::Restart { bind, log_file } => restart(bind, log_file, false).await,
            ServiceCommand::Logs { lines, follow } => logs(lines, follow),
            ServiceCommand::Status { json } => status(json).await,
        },
        Command::Connect { command } => match command {
            ConnectCommand::Codex(options) => install_codex(options).await,
            ConnectCommand::Ducx => {
                let executable = ensure_managed_ducx().await?;
                println!("managed ducx ready: {}", executable.display());
                Ok(())
            }
            ConnectCommand::Claude {
                settings_path,
                model,
            } => {
                let hook_settings_path = settings_path.clone();
                install_claude(settings_path, model)?;
                sync_claude_hooks(hook_settings_path)?;
                report_hook::sync_installation()
            }
            ConnectCommand::Dsh { dsh_home } => {
                let hooks_path = dsh_home
                    .clone()
                    .unwrap_or_else(dsh::default_dsh_home)
                    .join("hooks.json");
                install_dsh(dsh_home)?;
                report_hook::sync_installation_at(&hooks_path, report_hook::reporting_enabled()?)?;
                report_hook::sync_installation()
            }
            ConnectCommand::Status { settings_path } => claude_status(settings_path),
            ConnectCommand::Remove {
                target,
                settings_path,
                dsh_home,
            } => match target.as_str() {
                "codex" => uninstall_codex(None, None),
                "claude" => {
                    let hook_settings_path = settings_path.clone();
                    uninstall_claude(settings_path)?;
                    sync_claude_hooks(hook_settings_path)?;
                    report_hook::sync_installation()
                }
                "dsh" => {
                    let hooks_path = dsh_home
                        .clone()
                        .unwrap_or_else(dsh::default_dsh_home)
                        .join("hooks.json");
                    uninstall_dsh(dsh_home)?;
                    report_hook::sync_installation_at(
                        &hooks_path,
                        report_hook::reporting_enabled()?,
                    )?;
                    report_hook::sync_installation()
                }
                _ => unreachable!("clap validates connect target"),
            },
        },
        Command::Info { json } => status(json).await,
        Command::Fusion { command } => match command {
            FusionCommand::Get { id, json } => get_fusion_profile(id.as_deref(), json),
            FusionCommand::Set {
                profile_json,
                replace_id,
            } => set_fusion_profile(&profile_json, replace_id.as_deref()),
            FusionCommand::Delete { id } => delete_fusion_profile(id.as_deref()),
        },
        Command::Benchmark { command } => match command {
            BenchmarkCommand::Status => benchmark_status().await,
            BenchmarkCommand::Start {
                timeout_seconds,
                target_output_tokens,
                providers,
                models,
            } => benchmark_start(timeout_seconds, target_output_tokens, providers, models).await,
        },
        Command::Doctor {
            json,
            fix,
            restart_apps,
            quick,
        } => doctor(json, fix, restart_apps, quick).await,
        Command::Status { json } => status(json).await,
        Command::Models { json } => models(json).await,
        Command::Quota { json, provider } => quota(json, provider.as_deref()).await,
        Command::Usage { json } => usage(json).await,
        Command::Config { json, scope } => show_config(json, scope),
        Command::Start {
            bind,
            daemon,
            log_file,
        } => start(bind, daemon, log_file).await,
        Command::Stop { force } => stop(force),
        Command::Restart { bind, log_file } => restart(bind, log_file, false).await,
        Command::Logs { lines, follow } => logs(lines, follow),
        Command::Serve { bind } => start(bind, false, None).await,
        Command::Catalog { template_catalog } => {
            let config = GatewayConfig::from_stored_config()?;
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
            custom_only,
            config,
            catalog,
            base_url,
            web_search,
            env_key,
            no_env_key,
        } => {
            install_codex(InstallCodexOptions {
                requested_model: model,
                set_default: set_default || custom_only,
                codex_oauth_proxy,
                custom_only,
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
        Command::InstallClaude { settings, model } => {
            let hook_settings_path = settings.clone();
            install_claude(settings, model)?;
            sync_claude_hooks(hook_settings_path)?;
            report_hook::sync_installation()
        }
        Command::UninstallClaude { settings } => {
            let hook_settings_path = settings.clone();
            uninstall_claude(settings)?;
            sync_claude_hooks(hook_settings_path)?;
            report_hook::sync_installation()
        }
        Command::ClaudeStatus { settings } => claude_status(settings),
        Command::RefreshCodexCatalog => refresh_default_managed_codex_catalog().await,
        Command::ProbeWebSearch { force, json } => probe_web_search(force, json).await,
    }
}

#[cfg(test)]
mod tests;
