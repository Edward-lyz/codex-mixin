use std::collections::HashSet;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Value, json};

use codex_mixin::config::{GatewayConfig, stored_config_path};
use codex_mixin::gateway_access::GatewayClient;
use codex_mixin::provider::ProviderModel;

use super::official_models::{selected_catalog_models, selected_official_models};
use super::report_hook::reporting_enabled;
use super::runtime::effective_gateway_bind;
use codex_mixin::platform::write_atomic_if_changed;

const PI_PROVIDER_ID: &str = "codex-mixin";
const PI_API: &str = "openai-responses";
const PI_API_KEY_FILE: &str = "pi-api-key";
const PI_REPORT_EXTENSION_FILE: &str = "codex-mixin-report.ts";
const PI_REPORT_EXTENSION_MARKER: &str = "codex-mixin managed Pi DUCX reporting extension";

pub(in crate::cli) fn install_pi(agent_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::Pi;
    codex_mixin::application::client::install_with_client_key(client, || {
        let gateway_config = GatewayConfig::from_stored_config()?;
        let official_models = selected_official_models(&gateway_config)?;
        let agent_dir = resolve_pi_agent_dir(agent_dir)?;
        let models_path = agent_dir.join("models.json");
        let extension_path = agent_dir.join("extensions").join(PI_REPORT_EXTENSION_FILE);
        let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
        let bind = effective_gateway_bind(&gateway_config)?;
        install_pi_at(
            &models_path,
            &key_path,
            &extension_path,
            bind,
            &gateway_config,
            &official_models,
            reporting_enabled()?,
            true,
        )
        .map(|_| ())
    })
}

pub(in crate::cli) fn uninstall_pi(agent_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let agent_dir = resolve_pi_agent_dir(agent_dir)?;
    let models_path = agent_dir.join("models.json");
    let extension_path = agent_dir.join("extensions").join(PI_REPORT_EXTENSION_FILE);
    let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
    uninstall_pi_at(&models_path, &key_path, &extension_path)
}

pub(in crate::cli) fn pi_is_installed() -> anyhow::Result<bool> {
    let models_path = resolve_pi_agent_dir(None)?.join("models.json");
    let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
    codex_mixin::clients::pi::is_managed(&models_path, &key_path)
}

fn default_pi_agent_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    codex_mixin::platform::home_dir().join(".pi").join("agent")
}

pub(in crate::cli) fn resolve_pi_agent_dir(agent_dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    std::path::absolute(agent_dir.unwrap_or_else(default_pi_agent_dir)).map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
fn install_pi_at(
    models_path: &Path,
    key_path: &Path,
    extension_path: &Path,
    bind: SocketAddr,
    gateway_config: &GatewayConfig,
    official_models: &[ProviderModel],
    reporting_is_enabled: bool,
    announce: bool,
) -> anyhow::Result<bool> {
    let models = collect_pi_models(gateway_config, official_models);
    anyhow::ensure!(
        !models.is_empty(),
        "no enabled upstream models are available; refresh or select models before installing to Pi"
    );

    validate_pi_reporting_extension(extension_path)?;
    let gateway_key = gateway_config.require_client_key(GatewayClient::Pi)?;
    let changed =
        codex_mixin::clients::pi::install(models_path, key_path, bind, models, &gateway_key)?;
    sync_pi_reporting_extension(extension_path, reporting_is_enabled)?;

    if announce {
        println!("Pi models config updated: {}", models_path.display());
        println!("Pi provider: {PI_PROVIDER_ID}");
        println!("Pi API: {PI_API}");
        println!("Pi base URL: http://{bind}/v1");
        println!(
            "Pi reporting hooks: {}",
            if reporting_is_enabled {
                "installed"
            } else {
                "disabled"
            }
        );
        println!("reload required: run /reload in Pi or start a new Pi session");
    }
    Ok(changed)
}

fn uninstall_pi_at(
    models_path: &Path,
    key_path: &Path,
    extension_path: &Path,
) -> anyhow::Result<()> {
    validate_pi_reporting_extension(extension_path)?;
    codex_mixin::clients::pi::uninstall(models_path, key_path)?;
    sync_pi_reporting_extension(extension_path, false)?;

    println!("Pi models config restored: {}", models_path.display());
    println!("Pi provider removed: {PI_PROVIDER_ID}");
    println!("reload required: run /reload in Pi or start a new Pi session");
    Ok(())
}

fn collect_pi_models(config: &GatewayConfig, official_models: &[ProviderModel]) -> Vec<Value> {
    let catalog = selected_catalog_models(config, official_models);
    let mut seen: HashSet<String> = catalog.iter().map(|(id, _, _)| id.clone()).collect();
    let mut models: Vec<Value> = catalog
        .into_iter()
        .map(|(id, name, model)| pi_model(id, name, model, config))
        .collect();
    for profile in &config.fusion_profiles {
        let id = profile.model_slug();
        if !seen.insert(id.clone()) {
            continue;
        }
        models.push(json!({
            "id": id,
            "name": format!(
                "Fusion ({}): {} -> {}",
                profile.id,
                profile.panel_models.join("+"),
                profile.judge_model
            ),
            "reasoning": false,
            "input": ["text"],
            "cost": zero_cost(),
            "contextWindow": config.default_context_window,
            "maxTokens": config.default_max_tokens,
        }));
    }
    models
}

fn pi_model(id: String, name: String, model: &ProviderModel, config: &GatewayConfig) -> Value {
    let reasoning = model.supports_thinking != Some(false);
    let mut definition = json!({
        "id": id,
        "name": name,
        "reasoning": reasoning,
        "input": if model.supports_image == Some(true) {
            vec!["text", "image"]
        } else {
            vec!["text"]
        },
        "cost": zero_cost(),
        "contextWindow": model.context_window.unwrap_or(config.default_context_window),
        "maxTokens": config.default_max_tokens,
    });
    if reasoning {
        definition["thinkingLevelMap"] = json!({
            "off": "none",
            "minimal": "minimal",
            "low": "low",
            "medium": "medium",
            "high": "high",
            "xhigh": "xhigh",
            "max": "max",
        });
    }
    definition
}

fn zero_cost() -> Value {
    json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0})
}

pub(in crate::cli) fn sync_installed_pi_client_key() -> anyhow::Result<()> {
    let agent_dir = resolve_pi_agent_dir(None)?;
    let models_path = agent_dir.join("models.json");
    let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
    let synced = codex_mixin::application::client::sync_managed_client_key(
        GatewayClient::Pi,
        || codex_mixin::clients::pi::is_managed(&models_path, &key_path),
        |key| codex_mixin::clients::pi::sync_client_key(&key_path, key),
    )?;
    if !synced {
        return Ok(());
    }
    let extension_path = agent_dir.join("extensions").join(PI_REPORT_EXTENSION_FILE);
    sync_pi_reporting_extension(&extension_path, reporting_enabled()?)
}

pub(in crate::cli) fn sync_installed_pi_reporting() -> anyhow::Result<()> {
    let agent_dir = resolve_pi_agent_dir(None)?;
    let models_path = agent_dir.join("models.json");
    let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
    if !pi_provider_is_managed(&models_path, &key_path)? {
        return Ok(());
    }
    let extension_path = agent_dir.join("extensions").join(PI_REPORT_EXTENSION_FILE);
    sync_pi_reporting_extension(&extension_path, reporting_enabled()?)
}

/// Re-render the managed Pi provider entry from the current provider
/// configuration. A missing or unmanaged Pi agent dir is left untouched.
pub(in crate::cli) fn sync_installed_pi_models() -> anyhow::Result<bool> {
    let gateway_config = GatewayConfig::from_stored_config()?;
    let official_models = selected_official_models(&gateway_config)?;
    let bind = effective_gateway_bind(&gateway_config)?;
    let agent_dir = resolve_pi_agent_dir(None)?;
    let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
    sync_pi_models(
        &agent_dir,
        &key_path,
        bind,
        &gateway_config,
        &official_models,
        reporting_enabled()?,
    )
}

fn sync_pi_models(
    agent_dir: &Path,
    key_path: &Path,
    bind: SocketAddr,
    gateway_config: &GatewayConfig,
    official_models: &[ProviderModel],
    reporting_is_enabled: bool,
) -> anyhow::Result<bool> {
    let models_path = agent_dir.join("models.json");
    if !pi_provider_is_managed(&models_path, key_path)? {
        return Ok(false);
    }
    let extension_path = agent_dir.join("extensions").join(PI_REPORT_EXTENSION_FILE);
    install_pi_at(
        &models_path,
        key_path,
        &extension_path,
        bind,
        gateway_config,
        official_models,
        reporting_is_enabled,
        false,
    )
}

fn pi_provider_is_managed(models_path: &Path, key_path: &Path) -> anyhow::Result<bool> {
    codex_mixin::clients::pi::is_managed(models_path, key_path)
}

fn sync_pi_reporting_extension(path: &Path, enabled: bool) -> anyhow::Result<()> {
    validate_pi_reporting_extension(path)?;
    if path.exists() {
        if !enabled {
            fs::remove_file(path)
                .with_context(|| format!("remove Pi reporting extension {}", path.display()))?;
            return Ok(());
        }
    } else if !enabled {
        return Ok(());
    }

    let executable = codex_mixin::platform::installation::installed_cli_executable()?;
    let executable_json = serde_json::to_string(&executable.to_string_lossy())?;
    let extension = PI_REPORT_EXTENSION.replace("__MIXIN_EXECUTABLE__", &executable_json);
    write_atomic_if_changed(path, extension.as_bytes())?;
    Ok(())
}

fn validate_pi_reporting_extension(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let existing = fs::read_to_string(path)
        .with_context(|| format!("read Pi reporting extension {}", path.display()))?;
    anyhow::ensure!(
        existing.contains(PI_REPORT_EXTENSION_MARKER),
        "Pi reporting extension path is not managed by Codex Mixin: {}",
        path.display()
    );
    Ok(())
}

const PI_REPORT_EXTENSION: &str = r#"// codex-mixin managed Pi DUCX reporting extension
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { spawn } from "node:child_process";
import { writeFile, unlink } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

const executable = __MIXIN_EXECUTABLE__;
const provider = "codex-mixin";

function activeModel(ctx: ExtensionContext): string | undefined {
  return ctx.model?.provider === provider ? ctx.model.id : undefined;
}

async function report(event: string, body: Record<string, unknown>): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const child = spawn(executable, ["report-hook", "--event", event], {
      stdio: ["pipe", "ignore", "pipe"],
    });
    let errorOutput = "";
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => {
      if (errorOutput.length < 4096) errorOutput += chunk;
    });
    child.once("error", reject);
    child.once("close", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`codex-mixin Pi reporting failed for ${event}: ${errorOutput.trim()}`));
    });
    child.stdin.end(JSON.stringify(body));
  });
}

export default function (pi: ExtensionAPI) {
  pi.on("session_start", async (_event, ctx) => {
    const model = activeModel(ctx);
    if (!model) return;
    await report("session-start", {
      session_id: ctx.sessionManager.getSessionId(), model, cwd: ctx.cwd, client: "pi",
    });
  });

  pi.on("before_agent_start", async (event, ctx) => {
    const model = activeModel(ctx);
    if (!model) return;
    await report("user-prompt-submit", {
      session_id: ctx.sessionManager.getSessionId(), model, cwd: ctx.cwd,
      prompt: event.prompt, client: "pi",
    });
  });

  pi.on("turn_end", async (_event, ctx) => {
    const model = activeModel(ctx);
    if (!model) return;
    let transcriptPath = ctx.sessionManager.getSessionFile();
    let temporaryPath: string | undefined;
    if (!transcriptPath) {
      temporaryPath = join(tmpdir(), `codex-mixin-pi-${process.pid}-${Date.now()}.json`);
      await writeFile(temporaryPath, JSON.stringify(ctx.sessionManager.getEntries()), "utf8");
      transcriptPath = temporaryPath;
    }
    try {
      await report("stop", {
        session_id: ctx.sessionManager.getSessionId(), model, cwd: ctx.cwd,
        transcript_path: transcriptPath, client: "pi",
      });
    } finally {
      if (temporaryPath) await unlink(temporaryPath).catch(() => {});
    }
  });
}
"#;
