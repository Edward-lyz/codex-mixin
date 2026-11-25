use clap::{Args, Subcommand};
use codex_mixin::application::pi_plugins;
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Subcommand)]
pub(super) enum PluginCommand {
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
        #[arg(long, default_value = "")]
        args: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(super) struct RequiredPackage {
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
pub(super) enum PluginAuthCommand {
    /// Sign in to a provider from this terminal.
    Login {
        #[command(flatten)]
        account: ProviderAccount,
        #[arg(long)]
        no_browser: bool,
    },
    /// Remove a provider's stored sign-in credentials.
    Logout(ProviderAccount),
}

#[derive(Debug, Args)]
pub(super) struct ProviderAccount {
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

pub(super) async fn run(command: PluginCommand) -> anyhow::Result<()> {
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
        } => {
            let name = name.or(name_positional).unwrap_or_default();
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
    let (account, login) = match command {
        PluginAuthCommand::Login {
            account,
            no_browser,
        } => (account, Some(no_browser)),
        PluginAuthCommand::Logout(account) => (account, None),
    };
    let provider = account
        .provider
        .or(account.provider_positional)
        .unwrap_or_default();
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
