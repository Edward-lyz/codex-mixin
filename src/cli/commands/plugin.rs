use std::io::{self, BufRead, Read, Write};

use anyhow::{Context, ensure};
use clap::{Args, Subcommand};
use codex_mixin::application::pi_plugins;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum PluginCommand {
    /// Inspect the isolated runtime, installed packages, and loaded models.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Search for Pi npm packages.
    Search {
        #[arg(value_name = "QUERY", conflicts_with = "query")]
        query_positional: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Install an npm package and synchronize its models.
    Install(RequiredPackage),
    /// Read an installed npm package's metadata and README.
    Info(RequiredPackage),
    /// Remove an npm package and synchronize the remaining models.
    Remove(RequiredPackage),
    /// Update one package or all installed packages.
    Update {
        #[arg(value_name = "SPEC", conflicts_with = "spec")]
        spec_positional: Option<String>,
        #[arg(long)]
        spec: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Synchronize models from the isolated runtime with Mixin.
    Sync {
        #[arg(long)]
        json: bool,
    },
    /// Manage an isolated Pi provider account.
    Auth {
        #[command(subcommand)]
        command: PluginAuthCommand,
    },
    /// List extension commands available to the bridge.
    Commands {
        #[arg(long)]
        json: bool,
    },
    /// Run a command exposed by an installed Pi extension.
    RunCommand {
        #[arg(
            value_name = "NAME",
            conflicts_with = "name",
            required_unless_present = "name"
        )]
        name_positional: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "", allow_hyphen_values = true)]
        args: String,
        #[arg(long)]
        json: bool,
        /// Stream JSON lines to stdout and read prompt replies as JSON lines from stdin.
        #[arg(long, conflicts_with = "json")]
        events: bool,
    },
}

#[derive(Debug, Args)]
pub(in crate::cli) struct RequiredPackage {
    #[arg(
        value_name = "SPEC",
        conflicts_with = "spec",
        required_unless_present = "spec"
    )]
    spec_positional: Option<String>,
    #[arg(long)]
    spec: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum PluginAuthCommand {
    /// Sign in to a provider.
    Login {
        #[command(flatten)]
        account: ProviderAccount,
        #[arg(long)]
        no_browser: bool,
        /// Stream JSON lines to stdout and read prompt replies as JSON lines from stdin.
        #[arg(long, conflicts_with_all = ["json", "no_browser"])]
        events: bool,
    },
    /// Remove a provider's stored sign-in credentials.
    Logout(ProviderAccount),
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ProviderAccount {
    #[arg(
        value_name = "PROVIDER",
        conflicts_with = "provider",
        required_unless_present = "provider"
    )]
    provider_positional: Option<String>,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    json: bool,
}

pub(in crate::cli) async fn run(command: PluginCommand) -> anyhow::Result<()> {
    match command {
        PluginCommand::Status { json } => {
            let status = pi_plugins::status().await?;
            if json {
                print_json(&status)
            } else {
                print_status(status);
                Ok(())
            }
        }
        PluginCommand::Search {
            query_positional,
            query,
            limit,
            json,
        } => {
            let query = query.or(query_positional).unwrap_or_default();
            let packages = pi_plugins::search(&query, limit).await?;
            if json {
                return print_json(&packages);
            }
            for package in packages {
                let version = package.version.as_deref().unwrap_or("unknown version");
                let label = if package.installed {
                    " (installed)"
                } else {
                    ""
                };
                println!(
                    "{} {version}{label}\n  {}",
                    package.name, package.description
                );
            }
            Ok(())
        }
        PluginCommand::Install(package) => {
            let spec = package.spec.or(package.spec_positional).unwrap_or_default();
            let count = pi_plugins::install(&spec).await?;
            package_output(Some(&spec), count, package.json)
        }
        PluginCommand::Info(package) => {
            let spec = package.spec.or(package.spec_positional).unwrap_or_default();
            let info = pi_plugins::info(&spec).await?;
            if package.json {
                return print_json(&info);
            }
            println!(
                "{} {}",
                info.name,
                info.version.as_deref().unwrap_or("unknown version")
            );
            for text in [info.description, info.homepage, info.readme]
                .into_iter()
                .flatten()
            {
                println!("{text}");
            }
            Ok(())
        }
        PluginCommand::Remove(package) => {
            let spec = package.spec.or(package.spec_positional).unwrap_or_default();
            let count = pi_plugins::remove(&spec).await?;
            package_output(Some(&spec), count, package.json)
        }
        PluginCommand::Update {
            spec_positional,
            spec,
            json,
        } => {
            let spec = spec.or(spec_positional);
            let count = pi_plugins::update(spec.as_deref()).await?;
            package_output(spec.as_deref(), count, json)
        }
        PluginCommand::Sync { json } => {
            let count = pi_plugins::sync().await?;
            print_model_sync(&json!({ "synced_models": count }), count, json)
        }
        PluginCommand::Auth { command } => run_auth(command).await,
        PluginCommand::Commands { json } => {
            let commands = pi_plugins::list_commands().await?;
            if json {
                return print_json(&commands);
            }
            for command in commands {
                println!("{}\t{}", command.name, command.description);
            }
            Ok(())
        }
        PluginCommand::RunCommand {
            name_positional,
            name,
            args,
            json,
            events,
        } => {
            let name = name.or(name_positional).unwrap_or_default();
            if events {
                let mut emit = print_event;
                let result =
                    pi_plugins::run_command_interactive(&name, &args, interaction(&mut emit)?)
                        .await?;
                return emit(result);
            }
            let result = pi_plugins::run_command(&name, &args).await?;
            if json {
                print_json(&result)
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?);
                Ok(())
            }
        }
    }
}

async fn run_auth(command: PluginAuthCommand) -> anyhow::Result<()> {
    let (account, login, events) = match command {
        PluginAuthCommand::Login {
            account,
            no_browser,
            events,
        } => (account, Some(no_browser), events),
        PluginAuthCommand::Logout(account) => (account, None, false),
    };
    let provider = account
        .provider
        .or(account.provider_positional)
        .unwrap_or_default();
    if events {
        let mut emit = print_event;
        let count = pi_plugins::auth_login_interactive(&provider, interaction(&mut emit)?).await?;
        return emit(json!({ "type": "result", "provider": provider, "synced_models": count }));
    }
    let count = match login {
        Some(no_browser) => pi_plugins::auth_login(&provider, no_browser).await?,
        None => pi_plugins::auth_logout(&provider).await?,
    };
    print_model_sync(
        &json!({ "provider": provider, "synced_models": count }),
        count,
        account.json,
    )
}

fn interaction(
    on_event: &mut dyn FnMut(Value) -> anyhow::Result<()>,
) -> anyhow::Result<pi_plugins::Interaction<'_>> {
    const MAX_REPLY: u64 = 64 * 1024;
    let (send, replies) = mpsc::channel(32);
    // A blocking read may still be waiting when the session ends. Keep it off
    // Tokio's blocking pool so the runtime can shut down with stdin still open.
    std::thread::Builder::new()
        .name("pi-prompt-input".into())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            let mut line = Vec::new();
            loop {
                line.clear();
                let reply = (|| {
                    let count = (&mut stdin)
                        .take(MAX_REPLY + 1)
                        .read_until(b'\n', &mut line)?;
                    if count == 0 {
                        return Ok(None);
                    }
                    ensure!(
                        count as u64 <= MAX_REPLY,
                        "Pi prompt reply exceeds size limit"
                    );
                    Ok(Some(
                        serde_json::from_slice::<Value>(&line)
                            .context("invalid Pi prompt reply")?,
                    ))
                })();
                match reply {
                    Ok(Some(value)) => {
                        if send.blocking_send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = send.blocking_send(Err(error));
                        break;
                    }
                    Ok(None) => break,
                }
            }
        })
        .context("read Pi prompt replies from stdin")?;
    Ok(pi_plugins::Interaction { replies, on_event })
}

fn print_event(event: Value) -> anyhow::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &event)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn print_status(status: pi_plugins::Status) {
    println!(
        "Node.js: {}",
        status.node_version.as_deref().unwrap_or("unavailable")
    );
    println!(
        "Isolated Pi: {}",
        status.pi_version.as_deref().unwrap_or("uninitialized")
    );
    println!("npm registry: {}", status.npm_registry);
    println!("Packages: {}", status.installed_packages.len());
    for package in status.installed_packages {
        println!("  {package}");
    }
    println!("Providers: {}", status.discovered_providers.len());
    for provider in status.discovered_providers {
        println!("  {provider}");
    }
    println!("Models: {}", status.discovered_models.len());
    for model in status.discovered_models {
        println!("  {model}");
    }
}

fn package_output(spec: Option<&str>, count: usize, json_output: bool) -> anyhow::Result<()> {
    print_model_sync(
        &json!({ "spec": spec, "synced_models": count }),
        count,
        json_output,
    )
}

fn print_model_sync(
    value: &serde_json::Value,
    count: usize,
    json_output: bool,
) -> anyhow::Result<()> {
    if json_output {
        print_json(value)
    } else {
        println!("Synchronized Pi models: {count}");
        Ok(())
    }
}

fn print_json(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}
