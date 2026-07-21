use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use codex_mixin::catalog::{codex_catalog_from_models_with_metadata, load_template_catalog};
use codex_mixin::config::GatewayConfig;
use codex_mixin::server::AppState;

mod atomic_file;
mod auth;
mod codex;
mod config_input;
mod maintenance;
mod metadata;
mod runtime;
mod service;
mod status;

use auth::{login, logout};
use codex::{
    InstallCodexOptions, install_codex, refresh_default_managed_codex_catalog, uninstall_codex,
};
use maintenance::migrate_history;
use metadata::{load_model_metadata_resolver, refresh_metadata};
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
        Command::RefreshCodexCatalog => refresh_default_managed_codex_catalog().await,
        Command::ProbeWebSearch { force, json } => probe_web_search(force, json).await,
    }
}

#[cfg(test)]
mod tests;
