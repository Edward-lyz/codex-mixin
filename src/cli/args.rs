//! The clap command tree: every flag and subcommand the CLI accepts.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use codex_mixin::provider::ProviderPreset;

use super::codex::InstallCodexOptions;
use super::tui;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Connect custom model providers to Codex, Claude, DSH, OpenCode, and Pi"
)]
pub(super) struct Cli {
    /// Keep the plain CLI interface instead of opening the full-screen UI.
    #[arg(long, global = true)]
    pub(super) no_tui: bool,
    #[command(subcommand)]
    pub(super) command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Internal: report a Codex hook event to Baidu.
    #[command(hide = true)]
    ReportHook {
        #[arg(long)]
        event: String,
    },
    /// Internal: replay persisted Baidu reporting events.
    #[command(name = "report-replay", hide = true)]
    ReportReplay {
        #[arg(long)]
        all_sessions: bool,
        #[arg(long)]
        prepare_warmup: bool,
        #[arg(long)]
        json: bool,
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
    /// Install or remove an application integration.
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
        #[arg(long)]
        days: Option<u64>,
    },
    #[command(hide = true)]
    Config {
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum, default_value_t = ConfigScope::Effective)]
        scope: ConfigScope,
        #[arg(long, value_name = "PATH")]
        export: Option<PathBuf>,
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
pub(super) enum ServiceCommand {
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
pub(super) enum ConnectCommand {
    /// Install Codex integration.
    Codex(InstallCodexOptions),
    /// Install and sign in to the managed DUCX authentication carrier.
    Ducx,
    /// Install Claude Code integration.
    Claude {
        #[arg(long)]
        settings_path: Option<PathBuf>,
    },
    /// Install the Codex Mixin gateway as a DeepSeek Harness provider.
    Dsh {
        #[arg(long)]
        dsh_home: Option<PathBuf>,
    },
    /// Install the Codex Mixin gateway as an OpenCode provider.
    Opencode {
        #[arg(long = "config")]
        config_path: Option<PathBuf>,
    },
    /// Install the Codex Mixin gateway and reporting hooks into Pi.
    Pi {
        #[arg(long)]
        agent_dir: Option<PathBuf>,
    },
    /// Show Claude Code integration status.
    Status {
        #[arg(long)]
        settings_path: Option<PathBuf>,
    },
    /// Remove Codex, Claude, DSH, OpenCode, or Pi integration.
    Remove {
        #[arg(value_parser = ["codex", "claude", "dsh", "opencode", "pi"])]
        target: String,
        #[arg(long)]
        settings_path: Option<PathBuf>,
        #[arg(long)]
        dsh_home: Option<PathBuf>,
        #[arg(long = "opencode-config")]
        opencode_config: Option<PathBuf>,
        #[arg(long = "pi-agent-dir")]
        pi_agent_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum FusionCommand {
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
pub(super) enum BenchmarkCommand {
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
pub(super) enum ProviderCommand {
    /// List configured providers.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Add a provider from a preset.
    Add {
        #[arg(long, value_enum)]
        preset: CliProviderPreset,
        #[arg(long, value_name = "BOOL")]
        auxiliary_model_upstream: Option<bool>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        aws_access_key_id: Option<String>,
        #[arg(long)]
        aws_secret_access_key: Option<String>,
        #[arg(long)]
        aws_session_token: Option<String>,
        #[arg(long)]
        aws_region: Option<String>,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        website_url: Option<String>,
        #[arg(long, hide = true)]
        protocol: Option<String>,
        #[arg(long, hide = true)]
        api_path: Option<String>,
        #[arg(long, hide = true)]
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
        aws_access_key_id: Option<String>,
        #[arg(long)]
        aws_secret_access_key: Option<String>,
        #[arg(long)]
        aws_session_token: Option<String>,
        #[arg(long)]
        aws_region: Option<String>,
        #[arg(long, conflicts_with = "aws_session_token")]
        clear_aws_session_token: bool,
        #[arg(
            long,
            conflicts_with_all = ["aws_access_key_id", "aws_secret_access_key", "aws_session_token", "aws_region"]
        )]
        clear_aws_credentials: bool,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        website_url: Option<String>,
        #[arg(long, hide = true)]
        protocol: Option<String>,
        #[arg(long, hide = true)]
        api_path: Option<String>,
        #[arg(long, hide = true)]
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
    /// Set the complete provider order.
    Reorder {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Refresh the provider model catalog.
    Discover { id: String },
    /// Probe capabilities for models currently added to Codex.
    Probe { id: String },
    /// Test provider authentication and model access.
    Test {
        id: String,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        aws_access_key_id: Option<String>,
        #[arg(long)]
        aws_secret_access_key: Option<String>,
        #[arg(long)]
        aws_session_token: Option<String>,
        #[arg(long)]
        aws_region: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long, value_name = "disabled|ducx_loopback")]
        baidu_auth_bridge: Option<String>,
        #[arg(long, value_name = "PATH")]
        ducx_executable: Option<PathBuf>,
    },
    /// Select models exposed by a provider.
    Select {
        id: String,
        #[arg(long = "model")]
        models: Vec<String>,
        #[arg(long = "model-context", value_name = "MODEL=TOKENS")]
        model_contexts: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ConfigScope {
    Stored,
    Effective,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum SetupCodexMode {
    Official,
    Custom,
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum CliProviderPreset {
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
    #[value(name = "aws-bedrock", alias = "amazon-bedrock")]
    AwsBedrock,
}

impl CliProviderPreset {
    pub(super) fn as_provider_preset(self) -> ProviderPreset {
        match self {
            Self::Custom => ProviderPreset::Custom,
            Self::BaiduOneApi => ProviderPreset::BaiduOneApi,
            Self::OpenRouter => ProviderPreset::OpenRouter,
            Self::DeepSeek => ProviderPreset::DeepSeek,
            Self::OpenCodeGo => ProviderPreset::OpenCodeGo,
            Self::AwsBedrock => ProviderPreset::AwsBedrock,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
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
            ProviderPreset::AwsBedrock => Self::AwsBedrock,
        }
    }
}

pub(super) fn requested_tui_start(cli: &Cli, interactive: bool) -> Option<tui::StartPage> {
    if cli.no_tui {
        None
    } else {
        match &cli.command {
            None => Some(tui::StartPage::Dashboard),
            Some(Command::Setup {
                preset: None,
                key: None,
                quota_username: None,
                codex_mode: None,
                no_start: false,
            }) if interactive => Some(tui::StartPage::Setup),
            _ => None,
        }
    }
}
