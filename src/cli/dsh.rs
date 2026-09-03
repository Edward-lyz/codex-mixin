use std::collections::HashSet;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_yaml::{Mapping, Value};

use codex_mixin::config::GatewayConfig;
use codex_mixin::gateway_access::GatewayClient;
use codex_mixin::provider::catalog_model_slug;

use super::atomic_file::{ensure_owner_only_dir, set_owner_only, write_atomic_if_changed};
use super::official_models::selected_official_models;
use super::runtime::effective_gateway_bind;

pub(in crate::cli) const DSH_PROVIDER_ID: &str = "codex-mixin";
const DSH_API_PROTOCOL: &str = "openai-responses";
const DSH_API_KEY_ENV: &str = "CODEX_MIXIN_GATEWAY_API_KEY";

pub(in crate::cli) fn default_dsh_home() -> PathBuf {
    std::env::var_os("DSH_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".dsh"))
                .unwrap_or_else(|| PathBuf::from(".dsh"))
        })
}

fn resolve_dsh_home(dsh_home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    std::path::absolute(dsh_home.unwrap_or_else(default_dsh_home)).map_err(Into::into)
}

pub(in crate::cli) fn install_dsh(dsh_home: Option<PathBuf>) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::Dsh;
    let key_existed = codex_mixin::config::gateway_client_key_exists(client)?;
    codex_mixin::config::ensure_gateway_client_key(client)?;
    let result = (|| {
        let gateway_config = GatewayConfig::from_stored_config()?;
        let official_models = selected_official_models(&gateway_config)?;
        let bind = effective_gateway_bind(&gateway_config)?;
        install_dsh_with_models(dsh_home, &gateway_config, &official_models, bind)
    })();
    super::rollback_new_client_key_on_error(result, client, key_existed)
}

#[cfg(test)]
pub(in crate::cli) fn install_dsh_with_config(
    dsh_home: Option<PathBuf>,
    gateway_config: &GatewayConfig,
) -> anyhow::Result<()> {
    install_dsh_with_models(dsh_home, gateway_config, &[], gateway_config.bind)
}

fn install_dsh_with_models(
    dsh_home: Option<PathBuf>,
    gateway_config: &GatewayConfig,
    official_models: &[codex_mixin::provider::ProviderModel],
    bind: std::net::SocketAddr,
) -> anyhow::Result<()> {
    let dsh_home = resolve_dsh_home(dsh_home)?;
    ensure_owner_only_dir(&dsh_home)?;
    let models = collect_dsh_models(gateway_config, official_models);
    anyhow::ensure!(
        !models.is_empty(),
        "no enabled upstream models are available; refresh or select models before installing to DSH"
    );
    let profile = build_dsh_provider_profile(&bind, models);
    let credential_value = gateway_config.require_client_key(GatewayClient::Dsh)?;

    let settings_path = dsh_home.join("settings.yaml");
    let mut settings = read_yaml_document(&settings_path, "DSH settings")?;
    let root = settings
        .as_mapping_mut()
        .context("DSH settings must be a YAML mapping")?;
    let llm_pi_ai = root
        .entry(Value::String("llm-pi-ai".to_owned()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .context("DSH settings llm-pi-ai must be a YAML mapping")?;
    let providers = llm_pi_ai
        .entry(Value::String("providers".to_owned()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .context("DSH settings llm-pi-ai.providers must be a YAML mapping")?;
    providers.insert(Value::String(DSH_PROVIDER_ID.to_owned()), profile);

    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml_document(&credentials_path, "DSH credentials")?;
    let credentials_root = credentials
        .as_mapping_mut()
        .context("DSH credentials must be a YAML mapping")?;
    credentials_root.insert(
        Value::String(DSH_API_KEY_ENV.to_owned()),
        Value::String(credential_value),
    );
    write_yaml_owner_only(&credentials_path, &credentials)?;
    write_yaml_owner_only(&settings_path, &settings)?;

    println!("DSH settings updated: {}", settings_path.display());
    println!("DSH provider: {DSH_PROVIDER_ID}");
    println!("DSH base URL: http://{bind}/v1");
    println!("reload required: restart DSH or start a new DSH session");
    Ok(())
}

pub(in crate::cli) fn uninstall_dsh(dsh_home: Option<PathBuf>) -> anyhow::Result<()> {
    let dsh_home = resolve_dsh_home(dsh_home)?;
    let settings_path = dsh_home.join("settings.yaml");
    if !settings_path.exists() {
        anyhow::bail!(
            "DSH settings are not managed by codex-mixin: {}",
            settings_path.display()
        );
    }
    let mut settings = read_yaml_document(&settings_path, "DSH settings")?;
    let root = settings
        .as_mapping_mut()
        .context("DSH settings must be a YAML mapping")?;
    let llm_pi_ai = root
        .get_mut(Value::String("llm-pi-ai".to_owned()))
        .and_then(Value::as_mapping_mut)
        .context("DSH settings llm-pi-ai must be a YAML mapping")?;
    let providers = llm_pi_ai
        .get_mut(Value::String("providers".to_owned()))
        .and_then(Value::as_mapping_mut)
        .context("DSH settings llm-pi-ai.providers must be a YAML mapping")?;
    let profile = providers
        .remove(Value::String(DSH_PROVIDER_ID.to_owned()))
        .context("DSH provider codex-mixin is not installed")?;

    if profile.get("apiKeyEnv").and_then(Value::as_str) == Some(DSH_API_KEY_ENV) {
        let credentials_path = dsh_home.join(".credentials.yaml");
        if credentials_path.exists() {
            let mut credentials = read_yaml_document(&credentials_path, "DSH credentials")?;
            let credentials_root = credentials
                .as_mapping_mut()
                .context("DSH credentials must be a YAML mapping")?;
            credentials_root.remove(Value::String(DSH_API_KEY_ENV.to_owned()));
            write_yaml_owner_only(&credentials_path, &credentials)?;
        }
    }

    if providers.is_empty() {
        llm_pi_ai.remove(Value::String("providers".to_owned()));
    }
    if llm_pi_ai.is_empty() {
        root.remove(Value::String("llm-pi-ai".to_owned()));
    }
    if root.is_empty() {
        fs::remove_file(&settings_path)
            .with_context(|| format!("remove empty DSH settings {}", settings_path.display()))?;
    } else {
        write_yaml_owner_only(&settings_path, &settings)?;
    }

    println!("DSH settings restored: {}", settings_path.display());
    println!("DSH provider removed: {DSH_PROVIDER_ID}");
    println!("reload required: restart DSH or start a new DSH session");
    Ok(())
}

fn collect_dsh_models(
    config: &GatewayConfig,
    official_models: &[codex_mixin::provider::ProviderModel],
) -> Vec<Value> {
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
            let mut entry = Mapping::new();
            entry.insert(Value::String("id".to_owned()), Value::String(id));
            entry.insert(
                Value::String("name".to_owned()),
                Value::String(format!("{upstream_model_id} · {}", provider.display_name)),
            );
            if cached.supports_thinking != Some(false) {
                entry.insert(
                    Value::String("reasoningEfforts".to_owned()),
                    dsh_reasoning_efforts(),
                );
            }
            if let Some(context_window) = cached.context_window {
                entry.insert(
                    Value::String("contextWindow".to_owned()),
                    Value::Number(context_window.into()),
                );
            }
            if cached.supports_image == Some(true) {
                entry.insert(
                    Value::String("input".to_owned()),
                    Value::Sequence(vec![
                        Value::String("text".to_owned()),
                        Value::String("image".to_owned()),
                    ]),
                );
            }
            models.push(Value::Mapping(entry));
        }
    }
    for model in official_models {
        if !seen.insert(model.id.clone()) {
            continue;
        }
        let mut entry = Mapping::new();
        entry.insert(
            Value::String("id".to_owned()),
            Value::String(model.id.clone()),
        );
        entry.insert(
            Value::String("name".to_owned()),
            Value::String(format!(
                "{} · OpenAI",
                model.display_name.as_deref().unwrap_or(&model.id)
            )),
        );
        if model.supports_thinking != Some(false) {
            entry.insert(
                Value::String("reasoningEfforts".to_owned()),
                dsh_reasoning_efforts(),
            );
        }
        if let Some(context_window) = model.context_window {
            entry.insert(
                Value::String("contextWindow".to_owned()),
                Value::Number(context_window.into()),
            );
        }
        if model.supports_image == Some(true) {
            entry.insert(
                Value::String("input".to_owned()),
                Value::Sequence(vec![
                    Value::String("text".to_owned()),
                    Value::String("image".to_owned()),
                ]),
            );
        }
        models.push(Value::Mapping(entry));
    }
    for profile in &config.fusion_profiles {
        let id = profile.model_slug();
        if !seen.insert(id.clone()) {
            continue;
        }
        let mut entry = Mapping::new();
        entry.insert(Value::String("id".to_owned()), Value::String(id));
        entry.insert(
            Value::String("name".to_owned()),
            Value::String(format!(
                "Fusion ({}): {} -> {}",
                profile.id,
                profile.panel_models.join("+"),
                profile.judge_model
            )),
        );
        models.push(Value::Mapping(entry));
    }
    models
}

fn dsh_reasoning_efforts() -> Value {
    Value::Mapping(
        [
            ("off", Value::Null),
            ("minimal", Value::String("low".to_owned())),
            ("low", Value::String("low".to_owned())),
            ("medium", Value::String("medium".to_owned())),
            ("high", Value::String("high".to_owned())),
            ("xhigh", Value::String("max".to_owned())),
            ("max", Value::String("max".to_owned())),
        ]
        .into_iter()
        .map(|(effort, value)| (Value::String(effort.to_owned()), value))
        .collect(),
    )
}

fn build_dsh_provider_profile(bind: &SocketAddr, models: Vec<Value>) -> Value {
    let mut profile = Mapping::new();
    profile.insert(
        Value::String("api".to_owned()),
        Value::String(DSH_API_PROTOCOL.to_owned()),
    );
    profile.insert(
        Value::String("displayName".to_owned()),
        Value::String("Codex Mixin".to_owned()),
    );
    profile.insert(
        Value::String("baseURL".to_owned()),
        Value::String(format!("http://{bind}/v1")),
    );
    profile.insert(
        Value::String("apiKeyEnv".to_owned()),
        Value::String(DSH_API_KEY_ENV.to_owned()),
    );
    profile.insert(Value::String("models".to_owned()), Value::Sequence(models));
    Value::Mapping(profile)
}

pub(in crate::cli) fn sync_installed_dsh_client_key() -> anyhow::Result<()> {
    let dsh_home = resolve_dsh_home(None)?;
    let settings_path = dsh_home.join("settings.yaml");
    if !settings_path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&settings_path)?;
    if !raw.contains("displayName: Codex Mixin") {
        return Ok(());
    }
    let settings = read_yaml_document(&settings_path, "DSH settings")?;
    if settings
        .get("llm-pi-ai")
        .and_then(|value| value.get("providers"))
        .and_then(|value| value.get(DSH_PROVIDER_ID))
        .and_then(|provider| provider.get("displayName"))
        .and_then(Value::as_str)
        != Some("Codex Mixin")
    {
        return Ok(());
    }
    let client_key = codex_mixin::config::ensure_gateway_client_key(
        codex_mixin::gateway_access::GatewayClient::Dsh,
    )?;
    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml_document(&credentials_path, "DSH credentials")?;
    credentials
        .as_mapping_mut()
        .context("DSH credentials must be a YAML mapping")?
        .insert(
            Value::String(DSH_API_KEY_ENV.to_owned()),
            Value::String(client_key),
        );
    write_yaml_owner_only(&credentials_path, &credentials)
}

fn read_yaml_document(path: &Path, label: &str) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(Value::Mapping(Mapping::new()));
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("read {label} {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Value::Mapping(Mapping::new()));
    }
    serde_yaml::from_str(&raw).with_context(|| format!("parse {label} {}", path.display()))
}

fn write_yaml_owner_only(path: &Path, value: &Value) -> anyhow::Result<()> {
    let contents = serde_yaml::to_string(value)
        .with_context(|| format!("serialize DSH YAML {}", path.display()))?;
    write_atomic_if_changed(path, contents.as_bytes())?;
    set_owner_only(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use codex_mixin::config::ThinkingMode;
    use codex_mixin::provider::ProviderModel;
    use serde_yaml::Value;

    use super::*;

    fn gateway_config(gateway_key: Option<&str>, image_model: bool, fusion: bool) -> GatewayConfig {
        let mut provider = codex_mixin::provider::custom_provider("custom", "upstream-key");
        provider.display_name = "Custom Provider".to_owned();
        provider.selected_models = vec!["vision-model".to_owned()];
        provider.cached_models = vec![ProviderModel {
            id: "vision-model".to_owned(),
            display_name: Some("Vision Model".to_owned()),
            context_window: Some(128_000),
            supports_image: Some(image_model),
            ..ProviderModel::default()
        }];
        GatewayConfig {
            bind: "127.0.0.1:8787".parse().unwrap(),
            providers: vec![provider],
            official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
            codex_auth_path: PathBuf::from("/tmp/auth.json"),
            gateway_api_key: gateway_key.map(str::to_owned),
            gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
                dsh: Some(gateway_key.unwrap_or("dsh-client-key").to_owned()),
                ..Default::default()
            },
            accept_codex_oauth: false,
            official_selected_models: None,
            default_max_tokens: 8192,
            default_context_window: 128_000,
            request_timeout: std::time::Duration::from_secs(30),
            thinking_mode: ThinkingMode::Auto,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search".to_owned(),
            web_search_max_uses: None,
            fusion_profiles: if fusion {
                vec![codex_mixin::fusion::FusionProfile {
                    id: "default".to_owned(),
                    panel_models: vec!["vision-model-custom".to_owned()],
                    judge_model: "vision-model-custom".to_owned(),
                    final_model: "vision-model-custom".to_owned(),
                    min_successful: 1,
                    max_completion_tokens: 8192,
                    timeout_ms: 120_000,
                    show_intermediate_results: true,
                    panel_tools: codex_mixin::fusion::PanelToolsConfig::default(),
                }]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn install_writes_provider_and_credentials_then_uninstall_restores_other_sections() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("settings.yaml");
        fs::write(
            &settings_path,
            "llm-pi-ai:\n  providers:\n    existing:\n      api: openai-completions\n      baseURL: https://existing.example/v1\n      models:\n        - id: keep\n",
        )
        .unwrap();
        let config = gateway_config(Some("gateway-secret"), true, false);

        install_dsh_with_config(Some(directory.path().to_owned()), &config).unwrap();

        let settings: Value =
            serde_yaml::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        let provider = &settings["llm-pi-ai"]["providers"]["codex-mixin"];
        assert_eq!(provider["api"].as_str().unwrap(), "openai-responses");
        assert_eq!(
            provider["baseURL"].as_str().unwrap(),
            "http://127.0.0.1:8787/v1"
        );
        assert_eq!(provider["apiKeyEnv"].as_str().unwrap(), DSH_API_KEY_ENV);
        let model = &provider["models"][0];
        assert_eq!(model["id"].as_str().unwrap(), "vision-model-custom");
        assert_eq!(model["contextWindow"].as_u64().unwrap(), 128_000);
        assert_eq!(model["reasoningEfforts"]["off"].as_null(), Some(()));
        assert_eq!(model["reasoningEfforts"]["medium"].as_str(), Some("medium"));
        assert_eq!(model["reasoningEfforts"]["xhigh"].as_str(), Some("max"));
        assert_eq!(
            model["input"],
            Value::Sequence(vec![
                Value::String("text".to_owned()),
                Value::String("image".to_owned()),
            ])
        );
        assert_eq!(
            settings["llm-pi-ai"]["providers"]["existing"]["api"],
            "openai-completions"
        );

        let credentials_path = directory.path().join(".credentials.yaml");
        let credentials: Value =
            serde_yaml::from_str(&fs::read_to_string(&credentials_path).unwrap()).unwrap();
        assert_eq!(
            credentials[DSH_API_KEY_ENV].as_str().unwrap(),
            "gateway-secret"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&credentials_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        uninstall_dsh(Some(directory.path().to_owned())).unwrap();

        let restored: Value =
            serde_yaml::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert!(
            restored["llm-pi-ai"]["providers"]
                .get("codex-mixin")
                .is_none()
        );
        assert_eq!(
            restored["llm-pi-ai"]["providers"]["existing"]["api"],
            "openai-completions"
        );
        let restored_credentials: Value =
            serde_yaml::from_str(&fs::read_to_string(&credentials_path).unwrap()).unwrap();
        assert!(restored_credentials.get(DSH_API_KEY_ENV).is_none());
    }

    #[test]
    fn install_uses_client_credential_for_keyless_gateway_and_includes_fusion_models() {
        let directory = tempfile::tempdir().unwrap();
        let config = gateway_config(None, false, true);

        install_dsh_with_config(Some(directory.path().to_owned()), &config).unwrap();

        let settings: Value = serde_yaml::from_str(
            &fs::read_to_string(directory.path().join("settings.yaml")).unwrap(),
        )
        .unwrap();
        let provider = &settings["llm-pi-ai"]["providers"]["codex-mixin"];
        let model_ids = provider["models"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(model_ids.contains(&"vision-model-custom"));
        assert!(model_ids.contains(&"mixin/fusion/default"));
        let credentials: Value = serde_yaml::from_str(
            &fs::read_to_string(directory.path().join(".credentials.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            credentials[DSH_API_KEY_ENV].as_str().unwrap(),
            "dsh-client-key"
        );
    }

    #[test]
    fn includes_selected_official_models_without_provider_suffixes() {
        let config = gateway_config(None, false, false);
        let models = collect_dsh_models(
            &config,
            &[ProviderModel {
                id: "gpt-5.6-sol".to_owned(),
                display_name: Some("GPT-5.6 Sol".to_owned()),
                context_window: Some(272_000),
                supports_thinking: Some(true),
                ..ProviderModel::default()
            }],
        );
        let official = models
            .iter()
            .find(|model| model["id"].as_str() == Some("gpt-5.6-sol"))
            .unwrap();

        assert_eq!(official["name"].as_str(), Some("GPT-5.6 Sol · OpenAI"));
        assert_eq!(official["contextWindow"].as_u64(), Some(272_000));
    }

    #[test]
    fn install_rejects_whitespace_only_gateway_key_before_writing_settings() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = gateway_config(Some("gateway-secret"), false, false);
        config.gateway_client_keys.dsh = Some("   ".to_owned());

        let error =
            install_dsh_with_config(Some(directory.path().to_owned()), &config).unwrap_err();

        assert!(error.to_string().contains("DSH client key"));
        assert!(!directory.path().join("settings.yaml").exists());
    }

    #[test]
    fn uninstall_bails_when_provider_is_not_installed() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("settings.yaml");
        fs::write(&settings_path, "llm-pi-ai:\n  providers:\n    other: {}\n").unwrap();

        let error = uninstall_dsh(Some(directory.path().to_owned())).unwrap_err();
        assert!(error.to_string().contains("not installed"));
    }
}
