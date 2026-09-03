use std::collections::HashSet;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Map, Value, json};

use codex_mixin::config::{GatewayConfig, stored_config_path};
use codex_mixin::gateway_access::GatewayClient;
use codex_mixin::provider::{ProviderModel, catalog_model_slug};

use super::atomic_file::{set_owner_only, write_atomic_if_changed, write_owner_only};
use super::official_models::selected_official_models;
use super::report_hook::reporting_enabled;
use super::runtime::effective_gateway_bind;

const PI_PROVIDER_ID: &str = "codex-mixin";
const PI_PROVIDER_NAME: &str = "Codex Mixin";
const PI_API: &str = "openai-responses";
const PI_API_KEY_FILE: &str = "pi-api-key";
const PI_REPORT_EXTENSION_FILE: &str = "codex-mixin-report.ts";
const PI_REPORT_EXTENSION_MARKER: &str = "codex-mixin managed Pi DUCX reporting extension";

pub(in crate::cli) fn install_pi(agent_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::Pi;
    let key_existed = codex_mixin::config::gateway_client_key_exists(client)?;
    codex_mixin::config::ensure_gateway_client_key(client)?;
    let result = (|| {
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
        )
    })();
    super::rollback_new_client_key_on_error(result, client, key_existed)
}

pub(in crate::cli) fn uninstall_pi(agent_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let agent_dir = resolve_pi_agent_dir(agent_dir)?;
    let models_path = agent_dir.join("models.json");
    let extension_path = agent_dir.join("extensions").join(PI_REPORT_EXTENSION_FILE);
    let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
    uninstall_pi_at(&models_path, &key_path, &extension_path)
}

fn default_pi_agent_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".pi/agent"),
        |home| PathBuf::from(home).join(".pi").join("agent"),
    )
}

fn resolve_pi_agent_dir(agent_dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
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
) -> anyhow::Result<()> {
    let models = collect_pi_models(gateway_config, official_models);
    anyhow::ensure!(
        !models.is_empty(),
        "no enabled upstream models are available; refresh or select models before installing to Pi"
    );

    let mut document = read_pi_models(models_path)?;
    let root = document.as_object_mut().context(format!(
        "Pi models config must be a JSON object: {}",
        models_path.display()
    ))?;
    let providers = root
        .entry("providers".to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context(format!(
            "Pi models config providers must be a JSON object: {}",
            models_path.display()
        ))?;
    let key_reference = pi_key_reference(key_path);
    if let Some(existing) = providers.get(PI_PROVIDER_ID) {
        anyhow::ensure!(
            is_managed_provider(existing, &key_reference),
            "Pi provider {PI_PROVIDER_ID} already exists and is not managed by Codex Mixin"
        );
    }
    validate_pi_reporting_extension(extension_path)?;
    providers.insert(
        PI_PROVIDER_ID.to_owned(),
        json!({
            "name": PI_PROVIDER_NAME,
            "baseUrl": format!("http://{bind}/v1"),
            "apiKey": key_reference,
            "api": PI_API,
            "models": models,
        }),
    );

    let gateway_key = gateway_config.require_client_key(GatewayClient::Pi)?;
    write_owner_only(key_path, gateway_key.as_bytes())?;
    write_json_config(models_path, &document)?;
    sync_pi_reporting_extension(extension_path, reporting_is_enabled)?;

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
    Ok(())
}

fn uninstall_pi_at(
    models_path: &Path,
    key_path: &Path,
    extension_path: &Path,
) -> anyhow::Result<()> {
    validate_pi_reporting_extension(extension_path)?;
    anyhow::ensure!(
        models_path.exists(),
        "Pi models config is not managed by Codex Mixin: {}",
        models_path.display()
    );
    let mut document = read_pi_models(models_path)?;
    let root = document.as_object_mut().context(format!(
        "Pi models config must be a JSON object: {}",
        models_path.display()
    ))?;
    let providers = root
        .get_mut("providers")
        .and_then(Value::as_object_mut)
        .context(format!(
            "Pi models config has no providers object: {}",
            models_path.display()
        ))?;
    let key_reference = pi_key_reference(key_path);
    let provider = providers
        .get(PI_PROVIDER_ID)
        .context("Pi provider codex-mixin is not installed")?;
    anyhow::ensure!(
        is_managed_provider(provider, &key_reference),
        "Pi provider {PI_PROVIDER_ID} is not managed by Codex Mixin"
    );
    providers.remove(PI_PROVIDER_ID);
    if providers.is_empty() {
        root.remove("providers");
    }
    write_json_config(models_path, &document)?;
    if key_path.exists() {
        fs::remove_file(key_path)
            .with_context(|| format!("remove Pi gateway key {}", key_path.display()))?;
    }
    sync_pi_reporting_extension(extension_path, false)?;

    println!("Pi models config restored: {}", models_path.display());
    println!("Pi provider removed: {PI_PROVIDER_ID}");
    println!("reload required: run /reload in Pi or start a new Pi session");
    Ok(())
}

fn read_pi_models(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(json!({"providers": {}}));
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read Pi models config {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(json!({"providers": {}}));
    }
    serde_json::from_str(&raw).with_context(|| format!("parse Pi models config {}", path.display()))
}

fn is_managed_provider(provider: &Value, key_reference: &str) -> bool {
    provider.get("name").and_then(Value::as_str) == Some(PI_PROVIDER_NAME)
        && provider.get("api").and_then(Value::as_str) == Some(PI_API)
        && provider.get("apiKey").and_then(Value::as_str) == Some(key_reference)
}

fn collect_pi_models(config: &GatewayConfig, official_models: &[ProviderModel]) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for provider in &config.providers {
        if !provider.enabled {
            continue;
        }
        for upstream_model_id in &provider.selected_models {
            let Some(cached) = provider
                .cached_models
                .iter()
                .find(|candidate| &candidate.id == upstream_model_id)
            else {
                continue;
            };
            let id = catalog_model_slug(upstream_model_id, &provider.id);
            if !seen.insert(id.clone()) {
                continue;
            }
            models.push(pi_model(
                id,
                format!("{upstream_model_id} · {}", provider.display_name),
                cached,
                config,
            ));
        }
    }
    for model in official_models {
        if !seen.insert(model.id.clone()) {
            continue;
        }
        models.push(pi_model(
            model.id.clone(),
            format!(
                "{} · OpenAI",
                model.display_name.as_deref().unwrap_or(&model.id)
            ),
            model,
            config,
        ));
    }
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

fn pi_key_reference(key_path: &Path) -> String {
    format!("!cat {}", shell_quote(&key_path.to_string_lossy()))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(in crate::cli) fn sync_installed_pi_client_key() -> anyhow::Result<()> {
    let agent_dir = resolve_pi_agent_dir(None)?;
    let models_path = agent_dir.join("models.json");
    if !models_path.exists() {
        return Ok(());
    }
    let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
    let document = read_pi_models(&models_path)?;
    let key_reference = pi_key_reference(&key_path);
    if !document
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(PI_PROVIDER_ID))
        .is_some_and(|provider| is_managed_provider(provider, &key_reference))
    {
        return Ok(());
    }
    let client_key = codex_mixin::config::ensure_gateway_client_key(
        codex_mixin::gateway_access::GatewayClient::Pi,
    )?;
    write_owner_only(&key_path, client_key.as_bytes())?;
    let extension_path = agent_dir.join("extensions").join(PI_REPORT_EXTENSION_FILE);
    sync_pi_reporting_extension(&extension_path, reporting_enabled()?)
}

pub(in crate::cli) fn sync_installed_pi_reporting() -> anyhow::Result<()> {
    let agent_dir = resolve_pi_agent_dir(None)?;
    let models_path = agent_dir.join("models.json");
    if !models_path.exists() {
        return Ok(());
    }
    let key_path = std::path::absolute(stored_config_path().with_file_name(PI_API_KEY_FILE))?;
    let document = read_pi_models(&models_path)?;
    let key_reference = pi_key_reference(&key_path);
    if !document
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(PI_PROVIDER_ID))
        .is_some_and(|provider| is_managed_provider(provider, &key_reference))
    {
        return Ok(());
    }
    let extension_path = agent_dir.join("extensions").join(PI_REPORT_EXTENSION_FILE);
    sync_pi_reporting_extension(&extension_path, reporting_enabled()?)
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

    let executable = if cfg!(target_os = "macos") {
        let app = PathBuf::from("/Applications/Codex Mixin.app/Contents/Resources/codex-mixin");
        if app.is_file() {
            app
        } else {
            std::env::current_exe().context("resolve codex-mixin executable")?
        }
    } else {
        std::env::current_exe().context("resolve codex-mixin executable")?
    };
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

fn write_json_config(path: &Path, document: &Value) -> anyhow::Result<()> {
    let existed = path.exists();
    let mut encoded = serde_json::to_vec_pretty(document)?;
    encoded.push(b'\n');
    write_atomic_if_changed(path, &encoded)?;
    if !existed {
        set_owner_only(path)?;
    }
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
const mutatingTools = new Set(["apply_patch", "edit", "write"]);

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

  pi.on("tool_call", async (event, ctx) => {
    const model = activeModel(ctx);
    if (!model || !mutatingTools.has(event.toolName)) return;
    await report("pre-tool-use", {
      session_id: ctx.sessionManager.getSessionId(), model, cwd: ctx.cwd,
      tool_name: "apply_patch", pi_tool_name: event.toolName,
      tool_input: event.input, client: "pi",
    });
  });

  pi.on("tool_result", async (event, ctx) => {
    const model = activeModel(ctx);
    if (!model || !mutatingTools.has(event.toolName)) return;
    await report("post-tool-use", {
      session_id: ctx.sessionManager.getSessionId(), model, cwd: ctx.cwd,
      tool_name: "apply_patch", pi_tool_name: event.toolName,
      tool_input: event.input, tool_output: event.content,
      is_error: event.isError, client: "pi",
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

#[cfg(test)]
mod tests {
    use codex_mixin::config::ThinkingMode;
    use codex_mixin::provider::custom_provider;

    use super::*;

    fn gateway_config() -> GatewayConfig {
        let mut provider = custom_provider("custom", "upstream-key");
        provider.display_name = "Custom Provider".to_owned();
        provider.selected_models = vec!["vision-model".to_owned()];
        provider.cached_models = vec![ProviderModel {
            id: "vision-model".to_owned(),
            context_window: Some(128_000),
            supports_image: Some(true),
            supports_thinking: Some(true),
            ..ProviderModel::default()
        }];
        GatewayConfig {
            bind: "127.0.0.1:8787".parse().unwrap(),
            providers: vec![provider],
            official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
            codex_auth_path: PathBuf::from("/tmp/auth.json"),
            gateway_api_key: None,
            gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
                pi: Some("pi-client-key".to_owned()),
                ..Default::default()
            },
            accept_codex_oauth: false,
            official_selected_models: None,
            default_max_tokens: 8192,
            default_context_window: 256_000,
            request_timeout: std::time::Duration::from_secs(30),
            thinking_mode: ThinkingMode::Auto,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search".to_owned(),
            web_search_max_uses: None,
            fusion_profiles: Vec::new(),
        }
    }

    #[test]
    fn install_writes_provider_hooks_and_restores_other_models() {
        let directory = tempfile::tempdir().unwrap();
        let models_path = directory.path().join("models.json");
        let key_path = directory.path().join("pi-api-key");
        let extension_path = directory.path().join("extensions/codex-mixin-report.ts");
        fs::write(
            &models_path,
            serde_json::to_vec_pretty(&json!({
                "providers": {"existing": {"name": "Existing", "models": []}},
                "other": true,
            }))
            .unwrap(),
        )
        .unwrap();

        install_pi_at(
            &models_path,
            &key_path,
            &extension_path,
            "127.0.0.1:9898".parse().unwrap(),
            &gateway_config(),
            &[],
            true,
        )
        .unwrap();

        let document: Value = serde_json::from_slice(&fs::read(&models_path).unwrap()).unwrap();
        let provider = &document["providers"][PI_PROVIDER_ID];
        assert_eq!(document["providers"]["existing"]["name"], "Existing");
        assert_eq!(document["other"], true);
        assert_eq!(provider["api"], PI_API);
        assert_eq!(provider["baseUrl"], "http://127.0.0.1:9898/v1");
        assert_eq!(provider["models"][0]["id"], "vision-model-custom");
        assert_eq!(provider["models"][0]["input"], json!(["text", "image"]));
        assert_eq!(provider["models"][0]["thinkingLevelMap"]["max"], "max");
        assert_eq!(fs::read_to_string(&key_path).unwrap(), "pi-client-key");
        let extension = fs::read_to_string(&extension_path).unwrap();
        assert!(extension.contains(PI_REPORT_EXTENSION_MARKER));
        assert!(extension.contains("before_agent_start"));
        assert!(extension.contains("tool_call"));
        assert!(extension.contains("tool_result"));
        assert!(extension.contains("turn_end"));

        uninstall_pi_at(&models_path, &key_path, &extension_path).unwrap();
        let restored: Value = serde_json::from_slice(&fs::read(&models_path).unwrap()).unwrap();
        assert_eq!(restored["providers"]["existing"]["name"], "Existing");
        assert_eq!(restored["other"], true);
        assert!(restored["providers"].get(PI_PROVIDER_ID).is_none());
        assert!(!key_path.exists());
        assert!(!extension_path.exists());
    }

    #[test]
    fn install_rejects_unmanaged_provider_and_extension_collisions() {
        let directory = tempfile::tempdir().unwrap();
        let models_path = directory.path().join("models.json");
        let key_path = directory.path().join("pi-api-key");
        let extension_path = directory.path().join("extensions/codex-mixin-report.ts");
        fs::write(
            &models_path,
            serde_json::to_vec(&json!({
                "providers": {PI_PROVIDER_ID: {"name": "User provider"}}
            }))
            .unwrap(),
        )
        .unwrap();

        let error = install_pi_at(
            &models_path,
            &key_path,
            &extension_path,
            gateway_config().bind,
            &gateway_config(),
            &[],
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not managed by Codex Mixin"));
        assert!(!key_path.exists());

        fs::write(
            &models_path,
            serde_json::to_vec(&json!({"providers": {"existing": {"models": []}}})).unwrap(),
        )
        .unwrap();
        let original_models = fs::read(&models_path).unwrap();
        fs::create_dir_all(extension_path.parent().unwrap()).unwrap();
        fs::write(&extension_path, "user extension").unwrap();
        let error = install_pi_at(
            &models_path,
            &key_path,
            &extension_path,
            gateway_config().bind,
            &gateway_config(),
            &[],
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not managed by Codex Mixin"));
        assert_eq!(fs::read(&models_path).unwrap(), original_models);
        assert!(!key_path.exists());
    }
}
