//! Source-verified magpie configuration schemas. Protocol and app API
//! restrictions are explicit capabilities, never successful placeholder writes.
use std::path::{Path, PathBuf};

use crate::clients::IntegrationStatus;
use serde_json::{Value, json};

mod alma;
mod aside;
mod cindy;
mod dotenv;
mod engine;
mod hanako;
mod native_http;
mod schemas;
mod zed;

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

pub fn limitation(_id: &str) -> Option<&'static str> {
    None
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
    let native_status = match id {
        "zed" => Some(zed::status(manifest)),
        "aside" => Some(aside::status(manifest)),
        "cindy" => Some(cindy::status(manifest)),
        "alma" => Some(alma::status(manifest)),
        "hanako" => Some(hanako::status(manifest)),
        _ => None,
    };
    if let Some(result) = native_status {
        return Ok(result.unwrap_or_else(|error| IntegrationStatus {
            state: "error",
            mode: Some("native_app"),
            detail: "Native integration could not be inspected.",
            error: Some(format!("{error:#}")),
        }));
    }
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
    match id {
        "zed" => return zed::install(manifest, base, key, models),
        "aside" => return aside::install(manifest, base, key, models),
        "cindy" => return cindy::install(manifest, base, key),
        "alma" => return alma::install(manifest, base, key, models),
        "hanako" => return hanako::install(manifest, base, key, models),
        _ => {}
    }
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
        "muse" => Some(json!({"META_API_KEY":key})),
        "commandcode" => Some(if cfg!(feature = "client-auth") {
            json!({"CMD_LOCAL_ONLY":"1","CODEX_MIXIN_COMMANDCODE_API_KEY":key})
        } else {
            json!({"CMD_LOCAL_ONLY":"1"})
        }),
        "fx" | "empryo" | "ante" if cfg!(feature = "client-auth") => {
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
) -> anyhow::Result<(std::collections::BTreeMap<String, String>, Option<String>)> {
    name(id)?;
    anyhow::ensure!(
        status(id, manifest)?.state == "managed",
        "connect {id} before starting a managed session"
    );
    let path = manifest.with_extension("launch.json");
    if !path.exists() {
        return Ok((std::collections::BTreeMap::new(), None));
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
                        "META_API_KEY",
                        "CMD_LOCAL_ONLY"
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
        .collect::<anyhow::Result<std::collections::BTreeMap<_, _>>>()?;
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
    match id {
        "zed" => zed::remove(manifest),
        "aside" => aside::remove(manifest),
        "cindy" => cindy::remove(manifest),
        "alma" => alma::remove(manifest),
        "hanako" => hanako::remove(manifest),
        _ => engine::remove(manifest),
    }
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

#[cfg(test)]
mod native_tests {
    use super::*;

    #[test]
    fn native_model_updates_preserve_vendor_credentials_and_unrelated_edits() {
        for id in ["cursor", "copilot", "devin"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory
                .path()
                .canonicalize()
                .unwrap()
                .join("settings.json");
            let manifest = directory
                .path()
                .canonicalize()
                .unwrap()
                .join("integration.json");
            let original = match id {
                "cursor" => json!({"model":{"modelId":"old"},"hasChangedDefaultModel":false}),
                "copilot" => json!({"model":"old","effortLevel":"low"}),
                _ => json!({"agent":{"model":"old","other":true}}),
            };
            let mut original = original;
            original["nativeAccount"] = json!("fixture-vendor-account");
            original["theme"] = json!("original");
            std::fs::write(&path, original.to_string()).unwrap();
            let effort = (id == "copilot").then_some("high");
            let result =
                native_model_at(id, &manifest, &path, Some("vendor-model"), effort).unwrap();
            assert_eq!(result["capability"], "native_models");
            assert_eq!(result["model"], "vendor-model");
            assert_eq!(result["managed"], true);
            let mut current = engine::read_config(&path, engine::Format::Json).unwrap();
            current["theme"] = json!("user-edited");
            std::fs::write(&path, current.to_string()).unwrap();
            native_model_at(id, &manifest, &path, Some("another-native-model"), effort).unwrap();
            assert!(native_model_at(id, &manifest, &path, Some(""), effort).is_err());
            assert!(native_model_at(id, &manifest, &path, Some("bad\nmodel"), effort).is_err());
            remove(id, &manifest).unwrap();
            original["theme"] = json!("user-edited");
            assert_eq!(
                engine::read_config(&path, engine::Format::Json).unwrap(),
                original
            );
            assert!(!manifest.with_extension("launch.json").exists());
        }
    }

    #[test]
    fn private_launch_environment_requires_managed_fields_and_allowlisted_variables() {
        for variable in ["CODEX_MIXIN_FX_API_KEY", "LD_PRELOAD"] {
            let directory = tempfile::tempdir().unwrap();
            let manifest = directory
                .path()
                .canonicalize()
                .unwrap()
                .join("integration.json");
            let path = manifest.with_extension("launch.json");
            engine::apply(
                &manifest,
                vec![engine::Patch {
                    path: path.clone(),
                    format: engine::Format::Json,
                    fields: vec![(
                        vec![],
                        json!({"environment":{variable:"fixture-key"},"model":"provider/model"}),
                    )],
                }],
            )
            .unwrap();
            let result = launch_environment("fx", &manifest);
            if variable == "LD_PRELOAD" {
                assert!(result.is_err());
                continue;
            }
            let (environment, model) = result.unwrap();
            assert_eq!(environment[variable], "fixture-key");
            assert_eq!(model.as_deref(), Some("provider/model"));
            std::fs::write(
                path,
                r#"{"environment":{"CODEX_MIXIN_FX_API_KEY":"changed"}}"#,
            )
            .unwrap();
            assert!(launch_environment("fx", &manifest).is_err());
        }
    }
}
