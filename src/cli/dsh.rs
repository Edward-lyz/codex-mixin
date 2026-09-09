use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};

use codex_mixin::config::GatewayConfig;
use codex_mixin::gateway_access::GatewayClient;
use codex_mixin::provider::catalog_model_slug;

use super::official_models::selected_official_models;
use super::runtime::effective_gateway_bind;

pub(in crate::cli) const DSH_PROVIDER_ID: &str = "codex-mixin";
#[cfg(test)]
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
    codex_mixin::application::client::install_with_client_key(client, || {
        let gateway_config = GatewayConfig::from_stored_config()?;
        let official_models = selected_official_models(&gateway_config)?;
        let bind = effective_gateway_bind(&gateway_config)?;
        install_dsh_with_models(dsh_home, &gateway_config, &official_models, bind, true).map(|_| ())
    })
}

#[cfg(test)]
pub(in crate::cli) fn install_dsh_with_config(
    dsh_home: Option<PathBuf>,
    gateway_config: &GatewayConfig,
) -> anyhow::Result<()> {
    install_dsh_with_models(dsh_home, gateway_config, &[], gateway_config.bind, true).map(|_| ())
}

fn install_dsh_with_models(
    dsh_home: Option<PathBuf>,
    gateway_config: &GatewayConfig,
    official_models: &[codex_mixin::provider::ProviderModel],
    bind: std::net::SocketAddr,
    announce: bool,
) -> anyhow::Result<bool> {
    let dsh_home = resolve_dsh_home(dsh_home)?;
    let models = collect_dsh_models(gateway_config, official_models);
    anyhow::ensure!(
        !models.is_empty(),
        "no enabled upstream models are available; refresh or select models before installing to DSH"
    );
    let credential_value = gateway_config.require_client_key(GatewayClient::Dsh)?;
    let settings_path = dsh_home.join("settings.yaml");
    let changed = codex_mixin::clients::dsh::install(&dsh_home, bind, models, &credential_value)?;

    if announce {
        println!("DSH settings updated: {}", settings_path.display());
        println!("DSH provider: {DSH_PROVIDER_ID}");
        println!("DSH base URL: http://{bind}/v1");
        println!("reload required: restart DSH or start a new DSH session");
    }
    Ok(changed)
}

pub(in crate::cli) fn uninstall_dsh(dsh_home: Option<PathBuf>) -> anyhow::Result<()> {
    let dsh_home = resolve_dsh_home(dsh_home)?;
    let settings_path = dsh_home.join("settings.yaml");
    codex_mixin::clients::dsh::uninstall(&dsh_home)?;

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

pub(in crate::cli) fn sync_installed_dsh_client_key() -> anyhow::Result<()> {
    let dsh_home = resolve_dsh_home(None)?;
    codex_mixin::application::client::sync_managed_client_key(
        GatewayClient::Dsh,
        || codex_mixin::clients::dsh::is_managed(&dsh_home),
        |key| codex_mixin::clients::dsh::sync_client_key(&dsh_home, key),
    )?;
    Ok(())
}

/// Re-render the managed DSH provider profile from the current provider
/// configuration. A missing or unmanaged DSH home is left untouched.
pub(in crate::cli) fn sync_installed_dsh_models() -> anyhow::Result<bool> {
    let gateway_config = GatewayConfig::from_stored_config()?;
    let official_models = selected_official_models(&gateway_config)?;
    let bind = effective_gateway_bind(&gateway_config)?;
    sync_dsh_models(None, &gateway_config, &official_models, bind)
}

fn sync_dsh_models(
    dsh_home: Option<PathBuf>,
    gateway_config: &GatewayConfig,
    official_models: &[codex_mixin::provider::ProviderModel],
    bind: std::net::SocketAddr,
) -> anyhow::Result<bool> {
    let dsh_home = resolve_dsh_home(dsh_home)?;
    if !dsh_provider_is_managed(&dsh_home)? {
        return Ok(false);
    }
    install_dsh_with_models(Some(dsh_home), gateway_config, official_models, bind, false)
}

fn dsh_provider_is_managed(dsh_home: &Path) -> anyhow::Result<bool> {
    codex_mixin::clients::dsh::is_managed(dsh_home)
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
    fn sync_dsh_models_refreshes_only_a_managed_home() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("dsh");
        let config = gateway_config(Some("gateway-secret"), false, false);

        assert!(!sync_dsh_models(Some(home.clone()), &config, &[], config.bind).unwrap());
        assert!(!home.join("settings.yaml").exists());

        install_dsh_with_models(Some(home.clone()), &config, &[], config.bind, true).unwrap();
        assert!(!sync_dsh_models(Some(home.clone()), &config, &[], config.bind).unwrap());

        let mut updated = gateway_config(Some("gateway-secret"), false, false);
        updated.providers[0]
            .selected_models
            .push("second-model".to_owned());
        updated.providers[0].cached_models.push(ProviderModel {
            id: "second-model".to_owned(),
            ..ProviderModel::default()
        });
        assert!(sync_dsh_models(Some(home.clone()), &updated, &[], updated.bind).unwrap());
        let raw = fs::read_to_string(home.join("settings.yaml")).unwrap();
        assert!(raw.contains("second-model-custom"));
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
