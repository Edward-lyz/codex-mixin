use std::path::PathBuf;

use anyhow::Context;
use codex_mixin::application::agent_versions::{self, AGENT_VERSION_IDS};
use codex_mixin::application::agents::{agent_name, integration_status};
use serde_json::{Value, json};

use super::args::{AgentCommand, AgentConnectMode, ConnectCommand};

pub(super) async fn run(command: AgentCommand) -> anyhow::Result<()> {
    match command {
        AgentCommand::List { json } => {
            let workers = AGENT_VERSION_IDS
                .into_iter()
                .map(|id| tokio::task::spawn_blocking(move || snapshot(id)));
            let mut agents = Vec::new();
            for worker in futures_util::future::join_all(workers).await {
                agents.push(worker.context("agent inspection worker failed")??);
            }
            output(&json!({"agents": agents}), json)
        }
        AgentCommand::Versions { json, refresh } => output(
            &json!({"agents": agent_versions::check_versions(refresh).await}),
            json,
        ),
        AgentCommand::Upgrade { id, all, json } => {
            if all {
                return upgrade_all(json).await;
            }
            let id = id.context("an agent id or --all is required")?;
            let cli = agent_versions::upgrade_agent(&id).await?;
            output(&json!({"id": id, "cli": cli}), json)
        }
        AgentCommand::Connect { id, mode } => connect(&id, mode).await,
        AgentCommand::Disconnect { id } => disconnect(&id).await,
    }
}

fn output(value: &Value, json_output: bool) -> anyhow::Result<()> {
    if json_output {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}

fn integration_paths(id: &str) -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    let stored = std::path::absolute(codex_mixin::config::stored_config_path())?;
    let path = match id {
        "codex" => super::codex::resolve_codex_config_path(None)?,
        "claude" => super::claude::resolve_claude_settings_path(None)?,
        "claude-desktop" => codex_mixin::platform::claude_desktop_config_root()?,
        "dsh" => std::path::absolute(super::dsh::default_dsh_home())?,
        "opencode" => {
            return Ok((
                super::opencode::resolve_opencode_config_path(None)?,
                Some(stored.with_file_name("opencode-api-key")),
            ));
        }
        "pi" => {
            return Ok((
                super::pi::resolve_pi_agent_dir(None)?.join("models.json"),
                Some(stored.with_file_name("pi-api-key")),
            ));
        }
        _ => anyhow::bail!("unsupported agent: {id}"),
    };
    Ok((path, None))
}

fn snapshot(id: &str) -> anyhow::Result<Value> {
    let name = agent_name(id)?;
    let cli = agent_versions::discover_agent(id)?;
    let integration = match integration_paths(id) {
        Ok((path, key)) => integration_status(id, &path, key.as_deref())?,
        Err(error) => codex_mixin::application::agents::IntegrationStatus {
            state: "error",
            mode: None,
            detail: "Cannot resolve agent configuration paths.",
            error: Some(format!("{error:#}")),
        },
    };
    // Configuration presence and an installed program are independent facts.
    let detected = cli.path.is_some() || cli.app_version.is_some();
    Ok(
        json!({"id": id, "name": name, "detected": detected, "integration": integration, "cli": cli}),
    )
}

async fn upgrade_all(json_output: bool) -> anyhow::Result<()> {
    let mut results = Vec::new();
    for entry in agent_versions::check_versions(true).await {
        if let Some(error) = &entry.cli.error {
            results.push(json!({"id": entry.id, "cli": entry.cli, "error": error}));
            continue;
        }
        if !entry.cli.update {
            continue;
        }
        super::progress_step(&format!("Upgrading {}", entry.id));
        match agent_versions::upgrade_agent(&entry.id).await {
            Ok(cli) => results.push(json!({"id": entry.id, "error": cli.error, "cli": cli})),
            Err(error) => results.push(json!({"id": entry.id, "error": format!("{error:#}")})),
        }
    }
    // A batch has explicit per-agent outcomes, including partial completion.
    output(&json!({"results": results}), json_output)
}

fn integration_lock() -> anyhow::Result<std::fs::File> {
    let directory = super::runtime::state_dir();
    std::fs::create_dir_all(&directory)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("agent-integration.lock"))?;
    // The clients share gateway credentials/catalogs; serialize mutations.
    fs2::FileExt::try_lock_exclusive(&file)
        .context("another agent integration operation is running")?;
    Ok(file)
}

async fn connect(id: &str, mode: Option<AgentConnectMode>) -> anyhow::Result<()> {
    agent_name(id)?;
    let command = connect_command(id, mode)?;
    let _lock = integration_lock()?;
    super::service::ensure_ready().await?;
    super::progress_step("Applying agent configuration");
    // Retain existing install/auth/reporting transactions instead of duplicating them.
    Box::pin(super::run(super::Cli {
        no_tui: true,
        json_errors: true,
        command: Some(super::Command::Connect { command }),
    }))
    .await?;
    verify_integration(id, "managed").await
}

fn connect_command(id: &str, mode: Option<AgentConnectMode>) -> anyhow::Result<ConnectCommand> {
    anyhow::ensure!(
        id == "codex" || mode.is_none(),
        "--mode applies only to Codex"
    );
    Ok(match id {
        "codex" => {
            let mode =
                mode.context("Codex requires --mode official-account or --mode custom-only")?;
            ConnectCommand::Codex(super::codex::InstallCodexOptions {
                requested_model: None,
                set_default: mode == AgentConnectMode::CustomOnly,
                codex_oauth_proxy: mode == AgentConnectMode::OfficialAccount,
                custom_only: mode == AgentConnectMode::CustomOnly,
                config_path: None,
                catalog_path: None,
                base_url: None,
                web_search: "live".to_owned(),
                env_key: None,
                no_env_key: false,
            })
        }
        "claude" => ConnectCommand::Claude {
            settings_path: None,
        },
        "claude-desktop" => ConnectCommand::ClaudeDesktop {
            config_root: None,
            status: false,
            json: false,
        },
        "dsh" => ConnectCommand::Dsh { dsh_home: None },
        "opencode" => ConnectCommand::Opencode { config_path: None },
        "pi" => ConnectCommand::Pi { agent_dir: None },
        _ => anyhow::bail!("unsupported agent: {id}"),
    })
}

async fn disconnect(id: &str) -> anyhow::Result<()> {
    agent_name(id)?;
    let id = id.to_owned();
    let verified_id = id.clone();
    tokio::task::spawn_blocking(move || {
        let _lock = integration_lock()?;
        super::progress_step("Restoring agent configuration");
        super::remove_client_integration(&id, None, None, None, None, None)
    })
    .await
    .context("agent disconnect worker failed")??;
    // Do not use codex-switch official: it stops the shared gateway.
    verify_integration(&verified_id, "unmanaged").await
}

async fn verify_integration(id: &str, expected: &str) -> anyhow::Result<()> {
    let (path, key) = integration_paths(id)?;
    let state = integration_status(id, &path, key.as_deref())?;
    anyhow::ensure!(
        state.state == expected,
        "{id} integration did not become {expected}: {}",
        state.error.as_deref().unwrap_or(state.detail)
    );
    super::progress_step("Agent configuration verified");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_requires_explicit_mode_before_any_mutation() {
        assert!(connect_command("codex", None).is_err());
        assert!(connect_command("pi", Some(AgentConnectMode::CustomOnly)).is_err());
        assert!(connect_command("unknown", None).is_err());
        assert!(matches!(
            connect_command("pi", None).unwrap(),
            ConnectCommand::Pi { agent_dir: None }
        ));
    }
}
