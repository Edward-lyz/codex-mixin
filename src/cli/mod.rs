use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use codex_mixin::catalog::{codex_catalog_from_models_with_metadata, load_template_catalog};
use codex_mixin::config::GatewayConfig;
use codex_mixin::server::AppState;

mod atomic_file;
mod benchmark_proxy;
mod codex;
mod config_input;
mod fusion_config;
mod maintenance;
mod metadata;
mod providers;
mod runtime;
mod service;
mod status;

use benchmark_proxy::{benchmark_start, benchmark_status};
use codex::{
    InstallCodexOptions, install_codex, refresh_default_managed_codex_catalog, uninstall_codex,
};
use fusion_config::{get_fusion_profile, set_fusion_profile};
use maintenance::migrate_history;
use metadata::{load_model_metadata_resolver, refresh_metadata};
use providers::{
    AddProviderOptions, UpdateProviderOptions, add_provider, discover_models, list_providers,
    remove_provider, select_models, set_provider_enabled, test_provider, update_provider,
};
use service::{init_tracing, logs, restart, start, stop};
use status::{doctor, models, probe_web_search, quota, show_config, status};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Providers {
        #[command(subcommand)]
        command: Box<ProviderCommand>,
    },
    Fusion {
        #[command(subcommand)]
        command: FusionCommand,
    },
    Benchmark {
        #[command(subcommand)]
        command: BenchmarkCommand,
    },
    #[command(visible_alias = "check")]
    Doctor {
        #[arg(long)]
        json: bool,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Models {
        #[arg(long)]
        json: bool,
    },
    Quota {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        provider: Option<String>,
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
        #[arg(
            long,
            help = "Merge official GPT and custom models using Codex OpenAI auth; requires models_cache.json"
        )]
        codex_oauth_proxy: bool,
        #[arg(
            long,
            conflicts_with = "codex_oauth_proxy",
            help = "Install only custom upstream models without OpenAI auth or models_cache.json, and select a custom default model"
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
}

#[derive(Debug, Subcommand)]
enum BenchmarkCommand {
    Status,
    Start {
        #[arg(long)]
        timeout_seconds: u64,
        #[arg(long = "provider")]
        providers: Vec<String>,
        #[arg(long = "model")]
        models: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ProviderCommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Add {
        #[arg(long)]
        preset: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        key: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
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
        #[arg(long)]
        quota_currency: Option<String>,
        #[arg(long)]
        quota_parser: Option<String>,
        #[arg(long)]
        gateway_key: Option<String>,
        #[arg(long = "model")]
        static_models: Vec<String>,
    },
    Update {
        id: String,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, conflicts_with = "key")]
        clear_key: bool,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
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
        #[arg(long)]
        quota_currency: Option<String>,
        #[arg(long)]
        quota_parser: Option<String>,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Remove {
        id: String,
    },
    Discover {
        id: String,
    },
    Test {
        id: String,
        #[arg(long)]
        json: bool,
    },
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

pub(crate) async fn entrypoint() {
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
        Command::Providers { command } => match *command {
            ProviderCommand::List { json } => list_providers(json),
            ProviderCommand::Add {
                preset,
                id,
                key,
                display_name,
                base_url,
                protocol,
                api_path,
                models_path,
                image_generation_path,
                quota_url,
                quota_username,
                quota_currency,
                quota_parser,
                gateway_key,
                static_models,
            } => add_provider(AddProviderOptions {
                preset,
                id,
                key,
                display_name,
                base_url,
                protocol,
                api_path,
                models_path,
                image_generation_path,
                quota_url,
                quota_username,
                quota_currency,
                quota_parser,
                gateway_key,
                static_models,
            }),
            ProviderCommand::Update {
                id,
                key,
                clear_key,
                display_name,
                base_url,
                protocol,
                api_path,
                models_path,
                image_generation_path,
                clear_image_generation,
                quota_url,
                clear_quota,
                quota_username,
                quota_currency,
                quota_parser,
            } => update_provider(UpdateProviderOptions {
                id,
                key,
                clear_key,
                display_name,
                base_url,
                protocol,
                api_path,
                models_path,
                image_generation_path,
                clear_image_generation,
                quota_url,
                clear_quota,
                quota_username,
                quota_currency,
                quota_parser,
            }),
            ProviderCommand::Enable { id } => set_provider_enabled(&id, true),
            ProviderCommand::Disable { id } => set_provider_enabled(&id, false),
            ProviderCommand::Remove { id } => remove_provider(&id),
            ProviderCommand::Discover { id } => discover_models(&id).await,
            ProviderCommand::Test { id, json } => test_provider(&id, json).await,
            ProviderCommand::Select { id, models } => select_models(&id, models),
        },
        Command::Fusion { command } => match command {
            FusionCommand::Get { id, json } => get_fusion_profile(id.as_deref(), json),
            FusionCommand::Set {
                profile_json,
                replace_id,
            } => set_fusion_profile(&profile_json, replace_id.as_deref()),
        },
        Command::Benchmark { command } => match command {
            BenchmarkCommand::Status => benchmark_status().await,
            BenchmarkCommand::Start {
                timeout_seconds,
                providers,
                models,
            } => benchmark_start(timeout_seconds, providers, models).await,
        },
        Command::Doctor { json } => doctor(json).await,
        Command::Status { json } => status(json).await,
        Command::Models { json } => models(json).await,
        Command::Quota { json, provider } => quota(json, provider.as_deref()).await,
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
        Command::RefreshCodexCatalog => refresh_default_managed_codex_catalog().await,
        Command::ProbeWebSearch { force, json } => probe_web_search(force, json).await,
    }
}

#[cfg(test)]
mod tests;
