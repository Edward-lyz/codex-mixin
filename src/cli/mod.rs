use std::io::{self, IsTerminal};
use std::time::Instant;

use clap::Parser;
use codex_mixin::catalog::{codex_catalog_from_models_with_metadata, load_template_catalog};
use codex_mixin::config::GatewayConfig;
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
mod official_models;
mod opencode;
mod pi;
mod providers;
mod report_hook;
mod runtime;
mod service;
mod setup;
mod status;
mod tui;
mod update;

use benchmark_proxy::{benchmark_start, benchmark_status};
use claude::{
    claude_status, install_claude, sync_claude_hooks, sync_installed_claude_client_key,
    uninstall_claude,
};
use codex::{
    InstallCodexOptions, install_codex, refresh_default_managed_codex_catalog,
    sync_installed_codex_client_key, uninstall_codex,
};
use doctor::doctor;
use dsh::{install_dsh, sync_installed_dsh_client_key, uninstall_dsh};
use ducx_setup::ensure_managed_ducx;
use fusion_config::{delete_fusion_profile, get_fusion_profile, set_fusion_profile};
use maintenance::migrate_history;
use metadata::{load_model_metadata_resolver, refresh_metadata};
use opencode::{install_opencode, sync_installed_opencode_client_key, uninstall_opencode};
use pi::{install_pi, sync_installed_pi_client_key, uninstall_pi};
use providers::{
    AddProviderOptions, TestProviderOptions, UpdateProviderOptions, add_provider, discover_models,
    list_providers, probe_selected_models, remove_provider, reorder_providers, select_models,
    set_provider_enabled, test_provider, update_provider,
};
use service::{init_tracing, logs, restart, start, stop};
use status::{export_config, models, probe_web_search, quota, show_config, status, usage};

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

fn rollback_new_client_key_on_error(
    result: anyhow::Result<()>,
    client: codex_mixin::gateway_access::GatewayClient,
    key_existed: bool,
) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if key_existed => Err(error),
        Err(error) => match codex_mixin::config::revoke_gateway_client_key(client) {
            Ok(()) => Err(error),
            Err(revoke_error) => Err(anyhow::anyhow!(
                "{error:#}; gateway client key rollback also failed: {revoke_error:#}"
            )),
        },
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

mod args;
use args::*;

pub(crate) async fn entrypoint() {
    let cli = Cli::parse();
    let print_errors_to_stderr = matches!(&cli.command, Some(Command::ReportReplay { .. }));
    let tui_start = requested_tui_start(
        &cli,
        io::stdin().is_terminal() && io::stdout().is_terminal(),
    );
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
        Some(Command::ReportHook { .. } | Command::ReportReplay { .. }) => {
            Some(runtime::default_report_hook_log_path())
        }
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
    let result = if let Some(start_page) = tui_start {
        match setup::install_cli_command() {
            Ok(installed_path) => tui::run(start_page, installed_path).await,
            Err(error) => Err(error),
        }
    } else {
        run(cli).await
    };
    if let Err(error) = result {
        exit_with_command_error(error, foreground_log_file.is_some(), print_errors_to_stderr);
    }
}

fn exit_with_command_error(
    error: anyhow::Error,
    has_foreground_log: bool,
    print_to_stderr: bool,
) -> ! {
    if has_foreground_log {
        tracing::error!(error = %format!("{error:#}"), "command failed");
    }
    if !has_foreground_log || print_to_stderr {
        eprintln!("Error: {error:#}");
    }
    std::process::exit(1);
}

#[allow(clippy::cognitive_complexity)]
async fn run(cli: Cli) -> anyhow::Result<()> {
    if !matches!(
        &cli.command,
        Some(Command::ReportHook { .. } | Command::ReportReplay { .. })
    ) {
        sync_installed_codex_client_key()?;
        sync_installed_claude_client_key()?;
        sync_installed_dsh_client_key()?;
        sync_installed_opencode_client_key()?;
        sync_installed_pi_client_key()?;
    }
    match cli.command.unwrap_or(Command::Info { json: false }) {
        Command::ReportHook { event } => report_hook::run(&event).await,
        Command::ReportReplay {
            all_sessions,
            prepare_warmup,
            json,
        } => report_hook::replay(all_sessions, prepare_warmup, json).await,
        Command::Setup {
            preset,
            key,
            quota_username,
            codex_mode,
            no_start,
        } => setup::run(preset, key, quota_username, codex_mode, no_start).await,
        Command::Update => update::run().await,
        Command::Providers { command } => match *command {
            ProviderCommand::List { json } => list_providers(json),
            ProviderCommand::Add {
                preset,
                auxiliary_model_upstream,
                id,
                key,
                aws_access_key_id,
                aws_secret_access_key,
                aws_session_token,
                aws_region,
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
                    auxiliary_model_upstream,
                    id,
                    key,
                    aws_access_key_id,
                    aws_secret_access_key,
                    aws_session_token,
                    aws_region,
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
                aws_access_key_id,
                aws_secret_access_key,
                aws_session_token,
                aws_region,
                clear_aws_session_token,
                clear_aws_credentials,
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
                    aws_access_key_id,
                    aws_secret_access_key,
                    aws_session_token,
                    aws_region,
                    clear_aws_session_token,
                    clear_aws_credentials,
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
            ProviderCommand::Reorder { ids } => {
                reorder_providers(ids)?;
                report_hook::sync_installation()
            }
            ProviderCommand::Discover { id } => discover_models(&id).await,
            ProviderCommand::Probe { id } => probe_selected_models(&id).await,
            ProviderCommand::Test {
                id,
                json,
                key,
                aws_access_key_id,
                aws_secret_access_key,
                aws_session_token,
                aws_region,
                base_url,
                baidu_auth_bridge,
                ducx_executable,
            } => {
                let ducx_executable = match (baidu_auth_bridge.as_deref(), &ducx_executable) {
                    (Some("ducx_loopback"), None) => Some(ensure_managed_ducx().await?),
                    _ => ducx_executable,
                };
                test_provider(TestProviderOptions {
                    id,
                    json,
                    key,
                    aws_access_key_id,
                    aws_secret_access_key,
                    aws_session_token,
                    aws_region,
                    base_url,
                    baidu_auth_bridge,
                    ducx_executable,
                })
                .await
            }
            ProviderCommand::Select {
                id,
                models,
                model_contexts,
            } => select_models(&id, models, model_contexts),
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
            ConnectCommand::Claude { settings_path } => {
                let hook_settings_path = settings_path.clone();
                install_claude(settings_path)?;
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
            ConnectCommand::Opencode { config_path } => install_opencode(config_path),
            ConnectCommand::Pi { agent_dir } => install_pi(agent_dir),
            ConnectCommand::Status { settings_path } => claude_status(settings_path),
            ConnectCommand::Remove {
                target,
                settings_path,
                dsh_home,
                opencode_config,
                pi_agent_dir,
            } => match target.as_str() {
                "codex" => {
                    uninstall_codex(None, None)?;
                    codex_mixin::config::revoke_gateway_client_key(
                        codex_mixin::gateway_access::GatewayClient::Codex,
                    )
                }
                "claude" => {
                    let hook_settings_path = settings_path.clone();
                    uninstall_claude(settings_path)?;
                    codex_mixin::config::revoke_gateway_client_key(
                        codex_mixin::gateway_access::GatewayClient::Claude,
                    )?;
                    sync_claude_hooks(hook_settings_path)?;
                    report_hook::sync_installation()
                }
                "dsh" => {
                    let hooks_path = dsh_home
                        .clone()
                        .unwrap_or_else(dsh::default_dsh_home)
                        .join("hooks.json");
                    uninstall_dsh(dsh_home)?;
                    codex_mixin::config::revoke_gateway_client_key(
                        codex_mixin::gateway_access::GatewayClient::Dsh,
                    )?;
                    report_hook::sync_installation_at(
                        &hooks_path,
                        report_hook::reporting_enabled()?,
                    )?;
                    report_hook::sync_installation()
                }
                "opencode" => {
                    uninstall_opencode(opencode_config)?;
                    codex_mixin::config::revoke_gateway_client_key(
                        codex_mixin::gateway_access::GatewayClient::OpenCode,
                    )
                }
                "pi" => {
                    uninstall_pi(pi_agent_dir)?;
                    codex_mixin::config::revoke_gateway_client_key(
                        codex_mixin::gateway_access::GatewayClient::Pi,
                    )
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
        Command::Usage { json, days } => usage(json, days).await,
        Command::Config {
            json,
            scope,
            export,
        } => match export {
            Some(path) => export_config(&path),
            None => show_config(json, scope),
        },
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
        Command::UninstallCodex { config, catalog } => {
            uninstall_codex(config, catalog)?;
            codex_mixin::config::revoke_gateway_client_key(
                codex_mixin::gateway_access::GatewayClient::Codex,
            )
        }
        Command::InstallClaude { settings } => {
            let hook_settings_path = settings.clone();
            install_claude(settings)?;
            sync_claude_hooks(hook_settings_path)?;
            report_hook::sync_installation()
        }
        Command::UninstallClaude { settings } => {
            let hook_settings_path = settings.clone();
            uninstall_claude(settings)?;
            codex_mixin::config::revoke_gateway_client_key(
                codex_mixin::gateway_access::GatewayClient::Claude,
            )?;
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
