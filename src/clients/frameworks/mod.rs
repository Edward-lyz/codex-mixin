//! Source-verified magpie configuration schemas. Protocol and app API
//! restrictions are explicit capabilities, never successful placeholder writes.
use std::path::{Path, PathBuf};

use crate::application::agents::IntegrationStatus;
use serde_json::{Value, json};

mod engine;
mod schemas;

const PROVIDER: &str = "codex-mixin";
const DETAIL: &str = "Disconnect restores the saved managed fields and preserves unrelated settings. Restart the agent after changing its configuration. Edited managed fields require manual reconciliation before disconnect.";

pub struct FrameworkModel {
    pub id: String,
    pub name: String,
    pub context_window: u64,
    pub max_tokens: u64,
    pub supports_image: bool,
    pub protocol: crate::provider::ProviderProtocol,
    pub efforts: Vec<String>,
}

pub fn name(id: &str) -> anyhow::Result<&'static str> {
    Ok(match id {
        "gemini" => "Gemini CLI",
        "agy" => "agy",
        "openchamber" => "OpenChamber",
        "mimocode" => "MiMo Code",
        "aside" => "Aside",
        "omo" => "OmO",
        "goose" => "Goose",
        "cursor" => "Cursor CLI",
        "cursor-local" => "Cursor",
        "zed" => "Zed",
        "vscode" => "VS Code",
        "vscode-insiders" => "VS Code Insiders",
        "vscodium" => "VSCodium",
        "copilot-jetbrains" => "Copilot for JetBrains",
        "air" => "Air",
        "copilot" => "Copilot CLI",
        "crush" => "Crush",
        "reasonix" => "Reasonix",
        "commandcode" => "Command Code",
        "fx" => "fx",
        "omp" => "Oh My Pi",
        "devin" => "Devin",
        "hermes" => "Hermes Agent",
        "morph" => "Mister Morph",
        "kimi" => "Kimi Code",
        "qwen" => "Qwen Code",
        "muse" => "Muse",
        "empryo" => "Empryo",
        "ante" => "Ante",
        "minimax-code" => "MiniMax Code",
        "droid" => "Droid",
        "cline" => "Cline",
        "qoder" => "Qoder",
        "qoder-cn" => "Qoder CN",
        "grok" => "Grok Build",
        "zcode" => "ZCode",
        "workbuddy" => "WorkBuddy",
        "codebuddy" => "CodeBuddy",
        "pencil" => "Pencil",
        "t3code" => "T3 Code",
        "hanako" => "Hanako",
        "atomcode" => "AtomCode",
        "alma" => "Alma",
        "cindy" => "Cindy",
        _ => anyhow::bail!("unknown framework: {id}"),
    })
}

pub fn limitation(id: &str) -> Option<&'static str> {
    match id {
        "cursor" | "copilot" | "devin" => Some(
            "The vendor CLI exposes native model selection only; magpie does not connect it to a third-party gateway.",
        ),
        "gemini" | "agy" => Some(
            "Requires Gemini generateContent/streamGenerateContent wire protocol, which this gateway does not yet implement.",
        ),
        "zed" => Some(
            "Requires an OS credential-store adapter; writing settings alone does not install its API key.",
        ),
        "alma" => Some("Requires Alma's running local API for provider import and removal."),
        "cindy" => Some(
            "Requires cindy://provider/import with user confirmation; its provider database is encrypted.",
        ),
        "hanako" => {
            Some("Requires the running app API for its keychain-encrypted provider catalog.")
        }
        "aside" => Some("Requires Aside's native runtime connection and staging API."),
        "cursor-local" => {
            Some("Requires process-level environment injection and a managed Cursor launcher.")
        }
        "vscodium" => Some(
            "Requires installed Copilot Chat extension discovery to choose customendpoint or customoai correctly.",
        ),
        "muse" => Some(
            "Requires the Muse discovery endpoint /muse-code/models and an authenticated transport adapter.",
        ),
        "empryo" | "ante" | "commandcode" | "fx" => Some(
            "Requires a managed process launcher to authenticate without putting a gateway key in unsupported config fields.",
        ),
        "reasonix" => Some(
            "Requires an environment-file credential and coordinated planner/provider configuration adapter.",
        ),
        "minimax-code" => {
            Some("Requires MiniMax flattened model IDs and reversible gateway alias mapping.")
        }
        "cline" => Some(
            "Requires coordinated takeover of Cline's built-in OpenAI-compatible provider and credential store.",
        ),
        _ => None,
    }
}

pub fn require_supported(id: &str) -> anyhow::Result<()> {
    name(id)?;
    if let Some(reason) = limitation(id) {
        anyhow::bail!("{id}: {reason}");
    }
    Ok(())
}

pub fn manifest_path(id: &str, state: &Path) -> anyhow::Result<PathBuf> {
    name(id)?;
    Ok(state
        .join("framework-integrations")
        .join(format!("{id}.json")))
}

pub fn status(id: &str, manifest: &Path) -> anyhow::Result<IntegrationStatus> {
    name(id)?;
    if let Some(detail) = limitation(id) {
        return Ok(IntegrationStatus {
            state: "unsupported",
            mode: None,
            detail,
            error: None,
        });
    }
    Ok(match engine::is_managed(manifest) {
        Ok(managed) => IntegrationStatus {
            state: if managed { "managed" } else { "unmanaged" },
            mode: None,
            detail: DETAIL,
            error: None,
        },
        Err(error) => IntegrationStatus {
            state: if manifest.exists() { "drift" } else { "error" },
            mode: None,
            detail: DETAIL,
            error: Some(format!("{error:#}")),
        },
    })
}

pub fn install(
    id: &str,
    manifest: &Path,
    base: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<()> {
    require_supported(id)?;
    anyhow::ensure!(
        !models.is_empty(),
        "no selected models are available for {id}"
    );
    let patches = schemas::patches(id, base, key, models)?;
    engine::apply(manifest, patches)
}

pub fn remove(id: &str, manifest: &Path) -> anyhow::Result<()> {
    require_supported(id)?;
    engine::remove(manifest)
}

/// Configuration presence is useful detection for apps with no CLI. The
/// undo journal alone is never treated as evidence of an installed app.
pub fn detected(id: &str) -> anyhow::Result<bool> {
    let root = crate::platform::framework_app_root()?;
    let path = match id {
        "vscode" => root.join("Code/User"),
        "vscode-insiders" => root.join("Code - Insiders/User"),
        "vscodium" => root.join("VSCodium/User"),
        "air" => root.join("JetBrains/Air"),
        "openchamber" => home_path("OPENCHAMBER_DATA_DIR", ".config/openchamber")?,
        "zcode" => home_path("", ".zcode")?,
        "pencil" => home_path("", ".pencil")?,
        "t3code" => home_path("T3CODE_HOME", ".t3")?,
        "workbuddy" => home_path("WORKBUDDY_CONFIG_DIR", ".workbuddy")?,
        "grok" => home_path("GROK_HOME", ".grok")?,
        "morph" => home_path("MISTER_MORPH_CONFIG", ".morph/config.yaml")?,
        "copilot-jetbrains" => {
            crate::platform::copilot_config_root()?.join("github-copilot/byok.json")
        }
        _ => return Ok(false),
    };
    Ok(path.exists())
}

fn field(keys: &[&str], value: Value) -> (Vec<String>, Value) {
    (keys.iter().map(|s| (*s).to_owned()).collect(), value)
}

fn home_path(variable: &str, default: &str) -> anyhow::Result<PathBuf> {
    let path = match std::env::var_os(variable).filter(|v| !variable.is_empty() && !v.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => crate::platform::home_dir().join(default),
    };
    anyhow::ensure!(path.is_absolute(), "{variable} must be an absolute path");
    Ok(path)
}

fn config_home() -> anyhow::Result<PathBuf> {
    home_path("XDG_CONFIG_HOME", ".config")
}

fn pi_models(models: &[FrameworkModel], base: &str) -> Value {
    json!(models.iter().map(|m|json!({"id":m.id,"name":m.name,"api":match m.protocol {crate::provider::ProviderProtocol::OpenAiChat => "openai-completions",crate::provider::ProviderProtocol::OpenAiResponses => "openai-responses",crate::provider::ProviderProtocol::AnthropicMessages => "anthropic-messages"},"baseUrl":if m.protocol == crate::provider::ProviderProtocol::AnthropicMessages {base.trim_end_matches("/v1")} else {base},"input":if m.supports_image {vec!["text","image"]} else {vec!["text"]},"contextWindow":m.context_window,"maxTokens":m.max_tokens,"reasoning":!m.efforts.is_empty(),"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}})).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_covers_all_new_fixed_magpie_agents() {
        for id in crate::application::agent_versions::AGENT_VERSION_IDS {
            if ["codex", "claude", "claude-desktop", "dsh", "opencode", "pi"].contains(&id) {
                continue;
            }
            assert!(name(id).is_ok(), "{id}");
        }
        assert!(name("../../escape").is_err());
        assert!(require_supported("gemini").is_err());
    }
}
