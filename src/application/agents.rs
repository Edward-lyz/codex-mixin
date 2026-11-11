//! Agent integration state shared by all desktop shells. Paths are resolved
//! by the caller; configuration semantics stay in the existing client adapters.

use std::path::Path;

use anyhow::Context;

use crate::clients::{self, IntegrationStatus};

/// One built-in agent integration: its stable id, display name and what
/// disconnecting it does. Framework agents are listed in `clients::frameworks`.
pub struct AgentManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub disconnect_detail: &'static str,
}

pub const AGENTS: [AgentManifest; 6] = [
    AgentManifest {
        id: "codex",
        name: "Codex",
        disconnect_detail: "Disconnect resets config.toml to the official OpenAI provider (other settings are removed), removes the Mixin key and model catalog, and moves all history to openai. Restart Codex afterwards.",
    },
    AgentManifest {
        id: "claude",
        name: "Claude Code",
        disconnect_detail: "Disconnect restores Mixin-managed model, authentication and traffic settings, preserving other Claude Code settings. Start a new session afterwards.",
    },
    AgentManifest {
        id: "claude-desktop",
        name: "Claude Desktop",
        disconnect_detail: "Disconnect restores the saved Claude Desktop profile settings. Fully quit and reopen Claude Desktop afterwards.",
    },
    AgentManifest {
        id: "dsh",
        name: "DSH",
        disconnect_detail: "Disconnect removes the codex-mixin provider and its gateway credential from DSH, preserving other providers. Restart DSH afterwards.",
    },
    AgentManifest {
        id: "opencode",
        name: "OpenCode",
        disconnect_detail: "Disconnect removes the Mixin provider, credential and managed reporting plugin, preserving other OpenCode settings. Restart OpenCode afterwards.",
    },
    AgentManifest {
        id: "pi",
        name: "Pi",
        disconnect_detail: "Disconnect removes the Mixin provider, credential and managed reporting extension, preserving other Pi settings. Restart Pi afterwards.",
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
    let detail = manifest(id)
        .with_context(|| format!("unsupported agent: {id}"))?
        .disconnect_detail;
    let check = || -> anyhow::Result<(bool, Option<&'static str>)> {
        let managed = match id {
            "codex" => clients::codex::is_managed(config)?,
            "claude" => clients::claude::is_managed(config)?,
            "claude-desktop" => clients::claude_desktop::is_managed(config)?,
            "dsh" => clients::dsh::is_managed(config)?,
            "opencode" => {
                clients::opencode::is_managed(config, key.context("missing OpenCode key path")?)?
            }
            "pi" => clients::pi::is_managed(config, key.context("missing Pi key path")?)?,
            _ => anyhow::bail!("unsupported agent: {id}"),
        };
        if !managed || id != "codex" {
            return Ok((managed, None));
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
    };
    Ok(match check() {
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
