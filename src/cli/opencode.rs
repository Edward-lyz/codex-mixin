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

const OPENCODE_PROVIDER_ID: &str = "codex-mixin";
const OPENCODE_PROVIDER_NAME: &str = "Codex Mixin";
const OPENCODE_API_KEY_FILE: &str = "opencode-api-key";
const OPENCODE_SCHEMA: &str = "https://opencode.ai/config.json";
const OPENAI_RESPONSES_PACKAGE: &str = "@ai-sdk/openai";
const OPENCODE_REPORT_PLUGIN_FILE: &str = "codex-mixin-report.js";
const OPENCODE_REPORT_PLUGIN_MARKER: &str = "codex-mixin managed DUCX reporting plugin";

pub(in crate::cli) fn install_opencode(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::OpenCode;
    let key_existed = codex_mixin::config::gateway_client_key_exists(client)?;
    codex_mixin::config::ensure_gateway_client_key(client)?;
    let result = (|| {
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
        )
    })();
    super::rollback_new_client_key_on_error(result, client, key_existed)
}

pub(in crate::cli) fn uninstall_opencode(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config_path = resolve_opencode_config_path(config_path)?;
    let key_path = std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
    uninstall_opencode_at(&config_path, &key_path)
}

fn default_opencode_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OPENCODE_CONFIG").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|path| !path.is_empty()) {
        return PathBuf::from(path).join("opencode").join("opencode.json");
    }
    std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".config/opencode/opencode.json"),
        |home| {
            PathBuf::from(home)
                .join(".config")
                .join("opencode")
                .join("opencode.json")
        },
    )
}

fn resolve_opencode_config_path(config_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
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
) -> anyhow::Result<()> {
    let models = collect_opencode_models(gateway_config, official_models);
    anyhow::ensure!(
        !models.is_empty(),
        "no enabled upstream models are available; refresh or select models before installing to OpenCode"
    );

    let mut document = read_opencode_config(config_path)?;
    let root = document.as_object_mut().context(format!(
        "OpenCode config must be a JSON object: {}",
        config_path.display()
    ))?;
    root.entry("$schema".to_owned())
        .or_insert_with(|| Value::String(OPENCODE_SCHEMA.to_owned()));
    let providers = root
        .entry("provider".to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context(format!(
            "OpenCode config provider must be a JSON object: {}",
            config_path.display()
        ))?;
    let key_reference = format!("{{file:{}}}", key_path.display());
    if let Some(existing) = providers.get(OPENCODE_PROVIDER_ID) {
        anyhow::ensure!(
            is_managed_provider(existing, &key_reference),
            "OpenCode provider {OPENCODE_PROVIDER_ID} already exists and is not managed by Codex Mixin"
        );
    }

    providers.insert(
        OPENCODE_PROVIDER_ID.to_owned(),
        json!({
            "npm": OPENAI_RESPONSES_PACKAGE,
            "name": OPENCODE_PROVIDER_NAME,
            "options": {
                "baseURL": format!("http://{bind}/v1"),
                "apiKey": key_reference,
            },
            "models": models,
        }),
    );

    let gateway_key = gateway_config.require_client_key(GatewayClient::OpenCode)?;
    write_owner_only(key_path, gateway_key.as_bytes())?;
    write_json_config(config_path, &document)?;
    sync_opencode_reporting_plugin(config_path, reporting_is_enabled)?;

    println!("OpenCode config updated: {}", config_path.display());
    println!("OpenCode provider: {OPENCODE_PROVIDER_ID}");
    println!("OpenCode protocol package: {OPENAI_RESPONSES_PACKAGE}");
    println!("OpenCode base URL: http://{bind}/v1");
    println!("reload required: restart OpenCode or start a new OpenCode session");
    Ok(())
}

fn uninstall_opencode_at(config_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        config_path.exists(),
        "OpenCode config is not managed by Codex Mixin: {}",
        config_path.display()
    );
    let mut document = read_opencode_config(config_path)?;
    let root = document.as_object_mut().context(format!(
        "OpenCode config must be a JSON object: {}",
        config_path.display()
    ))?;
    let providers = root
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .context(format!(
            "OpenCode config has no provider object: {}",
            config_path.display()
        ))?;
    let key_reference = format!("{{file:{}}}", key_path.display());
    let provider = providers
        .get(OPENCODE_PROVIDER_ID)
        .context("OpenCode provider codex-mixin is not installed")?;
    anyhow::ensure!(
        is_managed_provider(provider, &key_reference),
        "OpenCode provider {OPENCODE_PROVIDER_ID} is not managed by Codex Mixin"
    );
    providers.remove(OPENCODE_PROVIDER_ID);
    if providers.is_empty() {
        root.remove("provider");
    }
    write_json_config(config_path, &document)?;
    if key_path.exists() {
        fs::remove_file(key_path)
            .with_context(|| format!("remove OpenCode gateway key {}", key_path.display()))?;
    }
    sync_opencode_reporting_plugin(config_path, false)?;

    println!("OpenCode config restored: {}", config_path.display());
    println!("OpenCode provider removed: {OPENCODE_PROVIDER_ID}");
    println!("reload required: restart OpenCode or start a new OpenCode session");
    Ok(())
}

fn is_managed_provider(provider: &Value, key_reference: &str) -> bool {
    provider.get("name").and_then(Value::as_str) == Some(OPENCODE_PROVIDER_NAME)
        && provider.get("npm").and_then(Value::as_str) == Some(OPENAI_RESPONSES_PACKAGE)
        && provider.pointer("/options/apiKey").and_then(Value::as_str) == Some(key_reference)
}

fn collect_opencode_models(
    config: &GatewayConfig,
    official_models: &[ProviderModel],
) -> Map<String, Value> {
    let mut seen = HashSet::new();
    let mut models = Map::new();
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
            models.insert(
                id,
                opencode_model(
                    format!("{upstream_model_id} · {}", provider.display_name),
                    cached,
                    config,
                ),
            );
        }
    }
    for model in official_models {
        if !seen.insert(model.id.clone()) {
            continue;
        }
        models.insert(
            model.id.clone(),
            opencode_model(
                format!(
                    "{} · OpenAI",
                    model.display_name.as_deref().unwrap_or(&model.id)
                ),
                model,
                config,
            ),
        );
    }
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
    if !config_path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&config_path)?;
    if !raw.contains(OPENCODE_PROVIDER_NAME) {
        return Ok(());
    }
    let key_path = std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
    let document = read_opencode_config(&config_path)?;
    let key_reference = format!("{{file:{}}}", key_path.display());
    if !document
        .get("provider")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(OPENCODE_PROVIDER_ID))
        .is_some_and(|provider| is_managed_provider(provider, &key_reference))
    {
        return Ok(());
    }
    let client_key = codex_mixin::config::ensure_gateway_client_key(
        codex_mixin::gateway_access::GatewayClient::OpenCode,
    )?;
    write_owner_only(&key_path, client_key.as_bytes())?;
    sync_opencode_reporting_plugin(&config_path, reporting_enabled()?)
}

pub(in crate::cli) fn sync_installed_opencode_reporting() -> anyhow::Result<()> {
    let config_path = resolve_opencode_config_path(None)?;
    if !config_path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&config_path)?;
    if !raw.contains(OPENCODE_PROVIDER_NAME) {
        return Ok(());
    }
    let key_path = std::path::absolute(stored_config_path().with_file_name(OPENCODE_API_KEY_FILE))?;
    let document = read_opencode_config(&config_path)?;
    let key_reference = format!("{{file:{}}}", key_path.display());
    if !document
        .get("provider")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(OPENCODE_PROVIDER_ID))
        .is_some_and(|provider| is_managed_provider(provider, &key_reference))
    {
        return Ok(());
    }
    sync_opencode_reporting_plugin(&config_path, reporting_enabled()?)
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
const mutatingTools = new Set(["apply_patch", "edit", "write"]);

async function report(event, body) {
  const child = Bun.spawn([executable, "report-hook", "--event", event], {
    stdin: JSON.stringify(body), stdout: "ignore", stderr: "ignore",
  });
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
  "tool.execute.before": async (input, output) => {
    const model = routes.get(input.sessionID);
    if (!model || !mutatingTools.has(input.tool)) return;
    await report("pre-tool-use", {
      session_id: input.sessionID, model, cwd: directory, tool_name: "apply_patch",
      opencode_tool_name: input.tool, tool_input: output.args, client: "opencode",
    });
  },
  "tool.execute.after": async (input, output) => {
    const model = routes.get(input.sessionID);
    if (!model || !mutatingTools.has(input.tool)) return;
    await report("post-tool-use", {
      session_id: input.sessionID, model, cwd: directory, tool_name: "apply_patch",
      opencode_tool_name: input.tool, tool_input: input.args,
      tool_output: output.output, client: "opencode",
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

fn read_opencode_config(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(json!({"$schema": OPENCODE_SCHEMA}));
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read OpenCode config {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(json!({"$schema": OPENCODE_SCHEMA}));
    }
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "parse OpenCode config as JSON {}; JSONC comments are not supported by this initial integration",
            path.display()
        )
    })
}

fn write_json_config(path: &Path, document: &Value) -> anyhow::Result<()> {
    let existed = path.exists();
    write_atomic_if_changed(path, &serde_json::to_vec_pretty(document)?)?;
    if !existed {
        set_owner_only(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use codex_mixin::config::ThinkingMode;
    use codex_mixin::provider::custom_provider;

    use super::*;

    fn gateway_config(gateway_key: Option<&str>) -> GatewayConfig {
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
            gateway_api_key: gateway_key.map(str::to_owned),
            gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
                opencode: Some(gateway_key.unwrap_or("opencode-client-key").to_owned()),
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
    fn install_writes_models_variants_and_restores_other_config() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("opencode.json");
        let key_path = directory.path().join("opencode-api-key");
        fs::write(
            &config_path,
            serde_json::to_vec_pretty(&json!({
                "$schema": OPENCODE_SCHEMA,
                "model": "existing/model",
                "plugin": ["keep-me"],
                "provider": {"existing": {"name": "Existing"}}
            }))
            .unwrap(),
        )
        .unwrap();
        let config = gateway_config(Some("gateway-secret"));
        let official = ProviderModel {
            id: "gpt-5.6-sol".to_owned(),
            display_name: Some("GPT-5.6 Sol".to_owned()),
            context_window: Some(272_000),
            supports_image: Some(true),
            supports_thinking: Some(true),
            ..ProviderModel::default()
        };

        install_opencode_with_models(
            &config_path,
            &key_path,
            "127.0.0.1:9898".parse().unwrap(),
            &config,
            &[official],
            false,
        )
        .unwrap();

        let document: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        let provider = &document["provider"][OPENCODE_PROVIDER_ID];
        assert_eq!(document["model"], "existing/model");
        assert_eq!(document["plugin"], json!(["keep-me"]));
        assert_eq!(document["provider"]["existing"]["name"], "Existing");
        assert_eq!(provider["npm"], OPENAI_RESPONSES_PACKAGE);
        assert_eq!(provider["options"]["baseURL"], "http://127.0.0.1:9898/v1");
        assert_eq!(
            provider["options"]["apiKey"],
            format!("{{file:{}}}", key_path.display())
        );
        let custom = &provider["models"]["vision-model-custom"];
        assert_eq!(custom["limit"]["context"], 128_000);
        assert_eq!(custom["limit"]["output"], 8192);
        assert_eq!(custom["modalities"]["input"], json!(["text", "image"]));
        assert_eq!(custom["variants"]["max"]["reasoningEffort"], "max");
        assert_eq!(
            provider["models"]["gpt-5.6-sol"]["name"],
            "GPT-5.6 Sol · OpenAI"
        );
        assert_eq!(fs::read_to_string(&key_path).unwrap(), "gateway-secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        uninstall_opencode_at(&config_path, &key_path).unwrap();

        let restored: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert!(restored["provider"].get(OPENCODE_PROVIDER_ID).is_none());
        assert_eq!(restored["provider"]["existing"]["name"], "Existing");
        assert_eq!(restored["model"], "existing/model");
        assert!(!key_path.exists());
    }

    #[test]
    fn install_without_gateway_key_uses_client_key() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("opencode.json");
        let key_path = directory.path().join("opencode-api-key");
        let config = gateway_config(None);

        install_opencode_with_models(&config_path, &key_path, config.bind, &config, &[], false)
            .unwrap();

        let document: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(
            document["provider"][OPENCODE_PROVIDER_ID]["npm"],
            OPENAI_RESPONSES_PACKAGE
        );
        assert_eq!(fs::read_to_string(key_path).unwrap(), "opencode-client-key");
    }

    #[test]
    fn install_rejects_an_unmanaged_provider_collision_without_writing_a_key() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("opencode.json");
        let key_path = directory.path().join("opencode-api-key");
        fs::write(
            &config_path,
            serde_json::to_vec(&json!({
                "provider": {OPENCODE_PROVIDER_ID: {"name": "User provider"}}
            }))
            .unwrap(),
        )
        .unwrap();
        let original = fs::read(&config_path).unwrap();
        let config = gateway_config(Some("gateway-secret"));

        let error =
            install_opencode_with_models(&config_path, &key_path, config.bind, &config, &[], false)
                .unwrap_err();

        assert!(error.to_string().contains("is not managed by Codex Mixin"));
        assert_eq!(fs::read(config_path).unwrap(), original);
        assert!(!key_path.exists());
    }

    #[test]
    fn reporting_plugin_installation_is_managed_and_reversible() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("opencode.json");
        sync_opencode_reporting_plugin(&config_path, true).unwrap();

        let plugin_path = directory
            .path()
            .join("plugins")
            .join(OPENCODE_REPORT_PLUGIN_FILE);
        let plugin = fs::read_to_string(&plugin_path).unwrap();
        assert!(plugin.contains(OPENCODE_REPORT_PLUGIN_MARKER));
        assert!(plugin.contains("chat.message"));
        assert!(plugin.contains("session.idle"));
        assert!(plugin.contains("tool.execute.before"));
        assert!(plugin.contains("tool.execute.after"));

        sync_opencode_reporting_plugin(&config_path, false).unwrap();
        assert!(!plugin_path.exists());
    }
}
