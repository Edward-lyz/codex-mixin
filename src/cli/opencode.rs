use std::collections::HashSet;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use codex_mixin::config::{GatewayConfig, stored_config_path};
use codex_mixin::gateway_access::GatewayClient;
use codex_mixin::provider::ProviderModel;

use super::official_models::{selected_catalog_models, selected_official_models};
use super::report_hook::reporting_enabled;
use super::runtime::effective_gateway_bind;
use codex_mixin::platform::write_atomic_if_changed;

const OPENCODE_PROVIDER_ID: &str = "codex-mixin";
const OPENCODE_API_KEY_FILE: &str = "opencode-api-key";
const OPENAI_RESPONSES_PACKAGE: &str = "@ai-sdk/openai";
const OPENCODE_REPORT_PLUGIN_FILE: &str = "codex-mixin-report.js";
const OPENCODE_REPORT_PLUGIN_MARKER: &str = "codex-mixin managed DUCX reporting plugin";

pub(in crate::cli) fn install_opencode(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::OpenCode;
    codex_mixin::application::client::install_with_client_key(client, || {
        let gateway_config = GatewayConfig::from_stored_config()?;
        let official_models = selected_official_models(&gateway_config)?;
        let config_path = resolve_opencode_config_path(config_path)?;
        let key_path =
            std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
        let bind = effective_gateway_bind(&gateway_config)?;
        install_opencode_with_models(
            &config_path,
            &key_path,
            bind,
            &gateway_config,
            &official_models,
            reporting_enabled()?,
            true,
        )
        .map(|_| ())
    })
}

pub(in crate::cli) fn uninstall_opencode(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config_path = resolve_opencode_config_path(config_path)?;
    let key_path = std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
    uninstall_opencode_at(&config_path, &key_path)
}

pub(in crate::cli) fn opencode_is_installed() -> anyhow::Result<bool> {
    let config_path = resolve_opencode_config_path(None)?;
    let key_path = std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
    codex_mixin::clients::opencode::is_managed(&config_path, &key_path)
}

fn default_opencode_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OPENCODE_CONFIG").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|path| !path.is_empty()) {
        return PathBuf::from(path).join("opencode").join("opencode.json");
    }
    codex_mixin::platform::home_dir()
        .join(".config")
        .join("opencode")
        .join("opencode.json")
}

pub(in crate::cli) fn resolve_opencode_config_path(
    config_path: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    std::path::absolute(config_path.unwrap_or_else(default_opencode_config_path))
        .map_err(Into::into)
}

fn install_opencode_with_models(
    config_path: &Path,
    key_path: &Path,
    bind: SocketAddr,
    gateway_config: &GatewayConfig,
    official_models: &[ProviderModel],
    reporting_is_enabled: bool,
    announce: bool,
) -> anyhow::Result<bool> {
    let models = collect_opencode_models(gateway_config, official_models);
    anyhow::ensure!(
        !models.is_empty(),
        "no enabled upstream models are available; refresh or select models before installing to OpenCode"
    );

    let gateway_key = gateway_config.require_client_key(GatewayClient::OpenCode)?;
    let changed =
        codex_mixin::clients::opencode::install(config_path, key_path, bind, models, &gateway_key)?;
    sync_opencode_reporting_plugin(config_path, reporting_is_enabled)?;

    if announce {
        println!("OpenCode config updated: {}", config_path.display());
        println!("OpenCode provider: {OPENCODE_PROVIDER_ID}");
        println!("OpenCode protocol package: {OPENAI_RESPONSES_PACKAGE}");
        println!("OpenCode base URL: http://{bind}/v1");
        println!("reload required: restart OpenCode or start a new OpenCode session");
    }
    Ok(changed)
}

fn uninstall_opencode_at(config_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    codex_mixin::clients::opencode::uninstall(config_path, key_path)?;
    sync_opencode_reporting_plugin(config_path, false)?;

    println!("OpenCode config restored: {}", config_path.display());
    println!("OpenCode provider removed: {OPENCODE_PROVIDER_ID}");
    println!("reload required: restart OpenCode or start a new OpenCode session");
    Ok(())
}

fn collect_opencode_models(
    config: &GatewayConfig,
    official_models: &[ProviderModel],
) -> Map<String, Value> {
    let catalog = selected_catalog_models(config, official_models);
    let mut seen: HashSet<String> = catalog.iter().map(|(id, _, _)| id.clone()).collect();
    let mut models: Map<String, Value> = catalog
        .into_iter()
        .map(|(id, name, model)| (id, opencode_model(name, model, config)))
        .collect();
    for profile in &config.fusion_profiles {
        let id = profile.model_slug();
        if !seen.insert(id.clone()) {
            continue;
        }
        models.insert(
            id,
            json!({
                "name": format!(
                    "Fusion ({}): {} -> {}",
                    profile.id,
                    profile.panel_models.join("+"),
                    profile.judge_model
                ),
                "limit": {
                    "context": config.default_context_window,
                    "output": config.default_max_tokens,
                },
                "modalities": {
                    "input": ["text"],
                    "output": ["text"],
                },
            }),
        );
    }
    models
}

fn opencode_model(name: String, model: &ProviderModel, config: &GatewayConfig) -> Value {
    let mut definition = json!({
        "name": name,
        "limit": {
            "context": model.context_window.unwrap_or(config.default_context_window),
            "output": config.default_max_tokens,
        },
        "modalities": {
            "input": if model.supports_image == Some(true) {
                vec!["text", "image"]
            } else {
                vec!["text"]
            },
            "output": ["text"],
        },
    });
    if model.supports_image == Some(true) {
        definition["attachment"] = Value::Bool(true);
    }
    if model.supports_thinking != Some(false) {
        definition["variants"] = opencode_reasoning_variants();
    }
    definition
}

fn opencode_reasoning_variants() -> Value {
    Value::Object(
        ["none", "low", "medium", "high", "xhigh", "max"]
            .into_iter()
            .map(|effort| {
                (
                    effort.to_owned(),
                    json!({
                        "reasoningEffort": effort,
                        "reasoningSummary": "auto",
                        "include": ["reasoning.encrypted_content"],
                    }),
                )
            })
            .collect(),
    )
}

pub(in crate::cli) fn sync_installed_opencode_client_key() -> anyhow::Result<()> {
    let config_path = resolve_opencode_config_path(None)?;
    let key_path = std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
    let synced = codex_mixin::application::client::sync_managed_client_key(
        GatewayClient::OpenCode,
        || codex_mixin::clients::opencode::is_managed(&config_path, &key_path),
        |key| codex_mixin::clients::opencode::sync_client_key(&key_path, key),
    )?;
    if !synced {
        return Ok(());
    }
    sync_opencode_reporting_plugin(&config_path, reporting_enabled()?)
}

pub(in crate::cli) fn sync_installed_opencode_reporting() -> anyhow::Result<()> {
    let config_path = resolve_opencode_config_path(None)?;
    let key_path = std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
    if !opencode_provider_is_managed(&config_path, &key_path)? {
        return Ok(());
    }
    sync_opencode_reporting_plugin(&config_path, reporting_enabled()?)
}

/// Re-render the managed OpenCode provider entry from the current provider
/// configuration. A missing or unmanaged OpenCode config is left untouched.
pub(in crate::cli) fn sync_installed_opencode_models() -> anyhow::Result<bool> {
    let gateway_config = GatewayConfig::from_stored_config()?;
    let official_models = selected_official_models(&gateway_config)?;
    let bind = effective_gateway_bind(&gateway_config)?;
    let config_path = resolve_opencode_config_path(None)?;
    let key_path = std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
    sync_opencode_models(
        &config_path,
        &key_path,
        bind,
        &gateway_config,
        &official_models,
        reporting_enabled()?,
    )
}

fn sync_opencode_models(
    config_path: &Path,
    key_path: &Path,
    bind: SocketAddr,
    gateway_config: &GatewayConfig,
    official_models: &[ProviderModel],
    reporting_is_enabled: bool,
) -> anyhow::Result<bool> {
    if !opencode_provider_is_managed(config_path, key_path)? {
        return Ok(false);
    }
    install_opencode_with_models(
        config_path,
        key_path,
        bind,
        gateway_config,
        official_models,
        reporting_is_enabled,
        false,
    )
}

fn opencode_provider_is_managed(config_path: &Path, key_path: &Path) -> anyhow::Result<bool> {
    codex_mixin::clients::opencode::is_managed(config_path, key_path)
}

fn sync_opencode_reporting_plugin(config_path: &Path, enabled: bool) -> anyhow::Result<()> {
    let config_directory = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("OpenCode config path has no parent"))?;
    let plugin_path = config_directory
        .join("plugins")
        .join(OPENCODE_REPORT_PLUGIN_FILE);
    if !enabled {
        if plugin_path.exists() {
            let raw = fs::read_to_string(&plugin_path)?;
            anyhow::ensure!(
                raw.contains(OPENCODE_REPORT_PLUGIN_MARKER),
                "OpenCode reporting plugin path is not managed by Codex Mixin"
            );
            fs::remove_file(&plugin_path)?;
        }
        return Ok(());
    }
    if plugin_path.exists() {
        let raw = fs::read_to_string(&plugin_path)?;
        anyhow::ensure!(
            raw.contains(OPENCODE_REPORT_PLUGIN_MARKER),
            "OpenCode reporting plugin path is not managed by Codex Mixin"
        );
    }
    let executable = codex_mixin::platform::installation::installed_cli_executable()?;
    let executable_json = serde_json::to_string(&executable.to_string_lossy())?;
    let plugin = OPENCODE_REPORT_PLUGIN.replace("__MIXIN_EXECUTABLE__", &executable_json);
    write_atomic_if_changed(&plugin_path, plugin.as_bytes())?;
    Ok(())
}

const OPENCODE_REPORT_PLUGIN: &str = r#"// codex-mixin managed DUCX reporting plugin
import { unlink } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

const executable = __MIXIN_EXECUTABLE__;
const routes = new Map();

async function report(event, body) {
  const child = Bun.spawn([executable, "report-hook", "--event", event], {
    stdin: "pipe", stdout: "ignore", stderr: "ignore",
  });
  child.stdin.write(JSON.stringify(body));
  child.stdin.end();
  if (await child.exited !== 0) {
    throw new Error(`codex-mixin DUCX reporting failed for ${event}`);
  }
}

export const CodexMixinReport = async ({ client, directory }) => ({
  "chat.message": async (input, output) => {
    if (input.model?.providerID !== "codex-mixin") return;
    const model = input.model.modelID;
    routes.set(input.sessionID, model);
    const prompt = output.parts.filter((part) => part.type === "text")
      .map((part) => part.text).join("\n");
    await report("user-prompt-submit", {
      session_id: input.sessionID, model, cwd: directory, prompt, client: "opencode",
    });
  },
  event: async ({ event }) => {
    if (event.type !== "session.idle") return;
    const sessionID = event.properties.sessionID;
    const model = routes.get(sessionID);
    if (!model) return;
    const transcriptPath = join(tmpdir(), `codex-mixin-opencode-${sessionID}.json`);
    try {
      const response = await client.session.messages({
        path: { id: sessionID }, query: { directory },
      });
      await Bun.write(transcriptPath, JSON.stringify(response.data ?? response));
      await report("stop", {
        session_id: sessionID, model, cwd: directory,
        transcript_path: transcriptPath, client: "opencode",
      });
    } finally {
      routes.delete(sessionID);
      await unlink(transcriptPath).catch(() => {});
    }
  },
});
"#;
