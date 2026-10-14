//! Agent integration state shared by all desktop shells. Paths are resolved
//! by the caller; configuration semantics stay in the existing client adapters.

use std::path::Path;

use anyhow::Context;
use serde::Serialize;

use crate::clients;

#[derive(Clone, Debug, Serialize)]
pub struct IntegrationStatus {
    pub state: &'static str,
    pub mode: Option<&'static str>,
    pub detail: &'static str,
    pub error: Option<String>,
}

pub fn agent_name(id: &str) -> anyhow::Result<&'static str> {
    match id {
        "codex" => Ok("Codex"),
        "claude" => Ok("Claude Code"),
        "claude-desktop" => Ok("Claude Desktop"),
        "dsh" => Ok("DSH"),
        "opencode" => Ok("OpenCode"),
        "pi" => Ok("Pi"),
        _ => clients::frameworks::name(id),
    }
}

pub fn integration_status(
    id: &str,
    config: &Path,
    key: Option<&Path>,
) -> anyhow::Result<IntegrationStatus> {
    agent_name(id)?;
    let detail = match id {
        "codex" => {
            "Disconnect restores the saved Codex configuration and authentication, migrates managed history back, and removes the managed model catalog. Restart Codex afterwards."
        }
        "claude" => {
            "Disconnect restores Mixin-managed model, authentication and traffic settings, preserving other Claude Code settings. Start a new session afterwards."
        }
        "claude-desktop" => {
            "Disconnect restores the saved Claude Desktop profile settings. Fully quit and reopen Claude Desktop afterwards."
        }
        "dsh" => {
            "Disconnect removes the codex-mixin provider and its gateway credential from DSH, preserving other providers. Restart DSH afterwards."
        }
        "opencode" => {
            "Disconnect removes the Mixin provider, credential and managed reporting plugin, preserving other OpenCode settings. Restart OpenCode afterwards."
        }
        "pi" => {
            "Disconnect removes the Mixin provider, credential and managed reporting extension, preserving other Pi settings. Restart Pi afterwards."
        }
        _ => anyhow::bail!("unsupported agent: {id}"),
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_mode_is_read_from_actual_managed_configuration() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::write(&path, "model_provider = 'codex-mixin'\n[model_providers.codex-mixin]\nbase_url = 'http://127.0.0.1:8787/v1'\n").unwrap();
        let state = integration_status("codex", &path, None).unwrap();
        assert_eq!(state.state, "managed");
        assert_eq!(state.mode, Some("codex_oauth_proxy"));
    }

    #[test]
    fn malformed_managed_configuration_remains_an_explicit_error() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        std::fs::write(&path, "# codex-mixin managed config\ninvalid = [").unwrap();
        let state = integration_status("codex", &path, None).unwrap();
        assert_eq!(state.state, "error");
        assert!(state.error.is_some());
        assert!(integration_status("unknown", &path, None).is_err());
    }
}
