//! Agent integration state shared by all desktop shells. Paths are resolved
//! by the caller; configuration semantics stay in the existing client adapters.

use std::path::Path;

use anyhow::Context;

use crate::clients::{self, IntegrationStatus};

type ManagedStatusCheck = fn(&Path, Option<&Path>) -> anyhow::Result<(bool, Option<&'static str>)>;

/// One built-in agent integration: its stable id, display name, disconnect
/// description, and managed-state probe. Framework agents are listed in
/// `clients::frameworks`.
pub struct AgentManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub disconnect_detail: &'static str,
    check_managed: ManagedStatusCheck,
}

fn check_codex_managed(
    config: &Path,
    _key: Option<&Path>,
) -> anyhow::Result<(bool, Option<&'static str>)> {
    let managed = clients::codex::is_managed(config)?;
    if !managed {
        return Ok((false, None));
    }
    let document = std::fs::read_to_string(config)?.parse::<toml_edit::DocumentMut>()?;
    let mode = match clients::codex::managed_provider_id(&document)? {
        crate::CODEX_MIXIN_PROVIDER => "codex_oauth_proxy",
        clients::codex::CUSTOM_ONLY_PROVIDER | clients::codex::LEGACY_CUSTOM_ONLY_PROVIDER => {
            "custom_only"
        }
        provider => anyhow::bail!("unknown managed Codex provider: {provider}"),
    };
    Ok((true, Some(mode)))
}

fn check_claude_managed(
    config: &Path,
    _key: Option<&Path>,
) -> anyhow::Result<(bool, Option<&'static str>)> {
    Ok((clients::claude::is_managed(config)?, None))
}

fn check_claude_desktop_managed(
    config: &Path,
    _key: Option<&Path>,
) -> anyhow::Result<(bool, Option<&'static str>)> {
    Ok((clients::claude_desktop::is_managed(config)?, None))
}

fn check_dsh_managed(
    config: &Path,
    _key: Option<&Path>,
) -> anyhow::Result<(bool, Option<&'static str>)> {
    Ok((clients::dsh::is_managed(config)?, None))
}

fn check_opencode_managed(
    config: &Path,
    key: Option<&Path>,
) -> anyhow::Result<(bool, Option<&'static str>)> {
    let managed = clients::opencode::is_managed(config, key.context("missing OpenCode key path")?)?;
    Ok((managed, None))
}

fn check_pi_managed(
    config: &Path,
    key: Option<&Path>,
) -> anyhow::Result<(bool, Option<&'static str>)> {
    let managed = clients::pi::is_managed(config, key.context("missing Pi key path")?)?;
    Ok((managed, None))
}

pub const AGENTS: [AgentManifest; 6] = [
    AgentManifest {
        id: "codex",
        name: "Codex",
        disconnect_detail: "Disconnect resets config.toml to the official OpenAI provider (other settings are removed), removes the Mixin key and model catalog, and moves all history to openai. Restart Codex afterwards.",
        check_managed: check_codex_managed,
    },
    AgentManifest {
        id: "claude",
        name: "Claude Code",
        disconnect_detail: "Disconnect restores Mixin-managed model, authentication and traffic settings, preserving other Claude Code settings. Start a new session afterwards.",
        check_managed: check_claude_managed,
    },
    AgentManifest {
        id: "claude-desktop",
        name: "Claude Desktop",
        disconnect_detail: "Disconnect restores the saved Claude Desktop profile settings. Fully quit and reopen Claude Desktop afterwards.",
        check_managed: check_claude_desktop_managed,
    },
    AgentManifest {
        id: "dsh",
        name: "DSH",
        disconnect_detail: "Disconnect removes the codex-mixin provider and its gateway credential from DSH, preserving other providers. Restart DSH afterwards.",
        check_managed: check_dsh_managed,
    },
    AgentManifest {
        id: "opencode",
        name: "OpenCode",
        disconnect_detail: "Disconnect removes the Mixin provider, credential and managed reporting plugin, preserving other OpenCode settings. Restart OpenCode afterwards.",
        check_managed: check_opencode_managed,
    },
    AgentManifest {
        id: "pi",
        name: "Pi",
        disconnect_detail: "Disconnect removes the Mixin provider, credential and managed reporting extension, preserving other Pi settings. Restart Pi afterwards.",
        check_managed: check_pi_managed,
    },
];

/// The built-in agent with this id, if any.
pub fn manifest(id: &str) -> Option<&'static AgentManifest> {
    AGENTS.iter().find(|agent| agent.id == id)
}

pub fn agent_name(id: &str) -> anyhow::Result<&'static str> {
    match manifest(id) {
        Some(agent) => Ok(agent.name),
        None => clients::frameworks::name(id),
    }
}

pub fn integration_status(
    id: &str,
    config: &Path,
    key: Option<&Path>,
) -> anyhow::Result<IntegrationStatus> {
    agent_name(id)?;
    let agent = manifest(id).with_context(|| format!("unsupported agent: {id}"))?;
    let detail = agent.disconnect_detail;
    Ok(match (agent.check_managed)(config, key) {
        Ok((managed, mode)) => IntegrationStatus {
            state: if managed { "managed" } else { "unmanaged" },
            mode,
            detail,
            error: None,
        },
        Err(error) => IntegrationStatus {
            state: "error",
            mode: None,
            detail,
            error: Some(format!(
                "read {id} integration at {}: {error:#}",
                config.display()
            )),
        },
    })
}
