//! Source-verified magpie configuration schemas. Protocol and app API
//! restrictions are explicit capabilities, never successful placeholder writes.
use std::path::{Path, PathBuf};

use crate::clients::IntegrationStatus;
use serde_json::{Value, json};

mod dotenv;
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
        "gemini" => Some(
            "Gemini Developer API ingress is available, but the CLI's topK/thinkingConfig and countTokens paths are not yet supported end to end.",
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
        "muse" => Some(
            "Requires the Muse discovery endpoint /muse-code/models and an authenticated transport adapter.",
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
    let native = matches!(id, "cursor" | "copilot" | "devin");
    if let Some(detail) = limitation(id).filter(|_| !manifest.exists()) {
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
            mode: native.then_some("native_models"),
            detail: if native {
                "Native vendor model management; this agent does not expose a custom gateway endpoint."
            } else {
                DETAIL
            },
            error: None,
        },
        Err(error) => IntegrationStatus {
            state: if manifest.exists() { "drift" } else { "error" },
            mode: native.then_some("native_models"),
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
    if manifest.exists() {
        anyhow::ensure!(engine::is_managed(manifest)?, "integration is not managed");
        return Ok(());
    }
    if matches!(id, "cursor" | "copilot" | "devin") {
        return engine::apply(manifest, Vec::new());
    }
    anyhow::ensure!(
        !models.is_empty(),
        "no selected models are available for {id}"
    );
    let patches = schemas::patches(id, base, key, models)?;
    let mut patches = patches;
    let environment = match id {
        "agy" => Some(json!({"GOOGLE_GEMINI_BASE_URL":base,"GEMINI_API_KEY":key})),
        "cursor-local" => Some(
            json!({"CURSOR_LOCAL_AGENT_BASE_URL":format!("{base}/v1"),"CURSOR_LOCAL_AGENT_API_KEY":key}),
        ),
        "muse" if cfg!(feature = "client-auth") => Some(json!({"META_API_KEY":key})),
        "commandcode" | "fx" | "empryo" | "ante" if cfg!(feature = "client-auth") => {
            Some(json!({format!("CODEX_MIXIN_{}_API_KEY",id.to_uppercase()):key}))
        }
        _ => None,
    };
    if let Some(environment) = environment {
        patches.push(engine::Patch {
            path: manifest.with_extension("launch.json"), format: engine::Format::Json,
            fields: vec![(vec![],json!({"environment":environment,"model":if id=="agy" {format!("{PROVIDER}/{}",models[0].id)} else {models[0].id.clone()}}))],
        });
    }
    engine::apply(manifest, patches)
}

/// Only managed launchers read this private record; callers never print it.
pub fn launch_environment(
    id: &str,
    manifest: &Path,
) -> anyhow::Result<(Vec<(String, String)>, Option<String>)> {
    name(id)?;
    anyhow::ensure!(
        status(id, manifest)?.state == "managed",
        "connect {id} before starting a managed session"
    );
    let path = manifest.with_extension("launch.json");
    if !path.exists() {
        return Ok((Vec::new(), None));
    }
    let record = engine::read_config(&path, engine::Format::Json)?;
    let environment = record["environment"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid managed launch environment"))?;
    let environment = environment
        .iter()
        .map(|(name, value)| {
            anyhow::ensure!(
                name.starts_with("CODEX_MIXIN_")
                    || [
                        "GOOGLE_GEMINI_BASE_URL",
                        "GEMINI_API_KEY",
                        "CURSOR_LOCAL_AGENT_BASE_URL",
                        "CURSOR_LOCAL_AGENT_API_KEY",
                        "META_API_KEY"
                    ]
                    .contains(&name.as_str()),
                "unexpected managed launch variable"
            );
            let value = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid managed launch value"))?;
            anyhow::ensure!(
                !value.chars().any(char::is_control),
                "invalid managed launch value"
            );
            Ok((name.clone(), value.to_owned()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok((
        environment,
        record
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned),
    ))
}

/// Read or change a vendor-native model without configuring a gateway or key.
pub fn native_model(
    id: &str,
    manifest: &Path,
    model: Option<&str>,
    effort: Option<&str>,
) -> anyhow::Result<Value> {
    let path = match id {
        "cursor" => home_path("", ".cursor/cli-config.json")?,
        "copilot" => home_path("", ".copilot/settings.json")?,
        "devin" => config_home()?.join("devin/config.json"),
        _ => anyhow::bail!("native model management requires cursor, copilot or devin"),
    };
    native_model_at(id, manifest, &path, model, effort)
}

fn native_model_at(
    id: &str,
    manifest: &Path,
    path: &Path,
    model: Option<&str>,
    effort: Option<&str>,
) -> anyhow::Result<Value> {
    use engine::{Format, Patch};
    anyhow::ensure!(
        matches!(id, "cursor" | "copilot" | "devin"),
        "unsupported native model manager"
    );
    anyhow::ensure!(
        model.is_some() || effort.is_none(),
        "--effort requires --model"
    );
    if let Some(model) = model {
        anyhow::ensure!(
            !model.trim().is_empty() && model.len() <= 256 && !model.chars().any(char::is_control),
            "invalid native model id"
        );
        anyhow::ensure!(
            id == "copilot" || effort.is_none(),
            "--effort applies only to Copilot; use a native model variant for Devin"
        );
        if let Some(effort) = effort {
            anyhow::ensure!(
                ["low", "medium", "high", "xhigh"].contains(&effort),
                "unsupported native reasoning effort"
            );
        }
        let fields = match id {
            "cursor" => vec![
                field(
                    &["model"],
                    json!({"modelId":model,"displayModelId":model,"displayName":model}),
                ),
                field(&["hasChangedDefaultModel"], json!(true)),
            ],
            "copilot" => {
                let mut fields = vec![field(&["model"], json!(model))];
                if let Some(effort) = effort {
                    fields.push(field(&["effortLevel"], json!(effort)));
                }
                fields
            }
            _ => vec![field(&["agent", "model"], json!(model))],
        };
        // The existing journal checks ownership before restoring. Reapply only
        // these native fields, retaining all unrelated changes and credentials.
        engine::remove(manifest)?;
        engine::apply(
            manifest,
            vec![Patch {
                path: path.to_owned(),
                format: Format::Json,
                fields,
            }],
        )?;
    }
    let value = engine::read_config(path, Format::Json)?;
    let selected = match id {
        "cursor" => value.pointer("/model/modelId"),
        "copilot" => value.get("model"),
        _ => value.pointer("/agent/model"),
    }
    .cloned()
    .unwrap_or(Value::Null);
    Ok(
        json!({"id":id,"capability":"native_models","model":selected,"effort":value.get("effortLevel"),"managed":engine::is_managed(manifest)?}),
    )
}

pub fn remove(id: &str, manifest: &Path) -> anyhow::Result<()> {
    name(id)?;
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
    let entries: Vec<_> = models
        .iter()
        .map(|m| {
            let api = match m.protocol {
                crate::provider::ProviderProtocol::OpenAiChat => "openai-completions",
                crate::provider::ProviderProtocol::OpenAiResponses => "openai-responses",
                crate::provider::ProviderProtocol::AnthropicMessages => "anthropic-messages",
                crate::provider::ProviderProtocol::GoogleGemini => "google-gemini",
            };
            let base_url = if m.protocol == crate::provider::ProviderProtocol::AnthropicMessages {
                base.trim_end_matches("/v1")
            } else {
                base
            };
            let input = if m.supports_image {
                vec!["text", "image"]
            } else {
                vec!["text"]
            };
            json!({
                "id": m.id,
                "name": m.name,
                "api": api,
                "baseUrl": base_url,
                "input": input,
                "contextWindow": m.context_window,
                "maxTokens": m.max_tokens,
                "reasoning": !m.efforts.is_empty(),
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
            })
        })
        .collect();
    json!(entries)
}
