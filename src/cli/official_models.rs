use std::collections::HashSet;
use std::path::{Path, PathBuf};

use codex_mixin::catalog::load_template_catalog;
use codex_mixin::config::{GatewayConfig, stored_config_path};
use codex_mixin::provider::{ProviderModel, ProviderProtocol};
use codex_mixin::server::AppState;
use serde_json::Value;

use super::atomic_file::write_atomic_if_changed;
use super::codex::{resolve_codex_client_version, resolve_codex_install_paths};

pub(super) const OFFICIAL_PROVIDER_ID: &str = "official";
const OFFICIAL_MODELS_CACHE_FILE: &str = "official-models.json";

pub(super) fn load_official_models() -> anyhow::Result<Vec<ProviderModel>> {
    let mixin_cache = official_models_cache_path();
    let codex_cache = resolve_codex_install_paths(None, None)?.models_cache;
    load_official_models_from_paths(&mixin_cache, &codex_cache)
}

pub(super) async fn refresh_official_models() -> anyhow::Result<usize> {
    let config = GatewayConfig::from_stored_config()?;
    anyhow::ensure!(
        config.accept_codex_oauth && config.codex_auth_path.is_file(),
        "official provider requires a signed-in Codex account"
    );
    let codex_cache = resolve_codex_install_paths(None, None)?.models_cache;
    let client_version = resolve_codex_client_version(&codex_cache)
        .ok_or_else(|| anyhow::anyhow!("Codex client version could not be determined"))?;
    refresh_official_models_to_path(&config, &client_version, &official_models_cache_path()).await
}

fn official_models_cache_path() -> PathBuf {
    stored_config_path().with_file_name(OFFICIAL_MODELS_CACHE_FILE)
}

fn load_official_models_from_paths(
    mixin_cache: &Path,
    codex_cache: &Path,
) -> anyhow::Result<Vec<ProviderModel>> {
    let cache = if mixin_cache.is_file() {
        mixin_cache
    } else {
        codex_cache
    };
    load_template_catalog(Some(cache))?.map_or_else(
        || Ok(Vec::new()),
        |catalog| official_models_from_catalog(&catalog),
    )
}

async fn refresh_official_models_to_path(
    config: &GatewayConfig,
    client_version: &str,
    cache_path: &Path,
) -> anyhow::Result<usize> {
    let state = AppState::new(config.clone())?;
    let catalog = state.fetch_official_models_catalog(client_version).await?;
    let models = official_models_from_catalog(&catalog)?;
    anyhow::ensure!(
        !models.is_empty(),
        "official models endpoint returned no visible models"
    );
    write_atomic_if_changed(cache_path, &serde_json::to_vec_pretty(&catalog)?)?;
    Ok(models.len())
}

pub(super) fn selected_official_models(
    config: &GatewayConfig,
) -> anyhow::Result<Vec<ProviderModel>> {
    if !config.accept_codex_oauth || !config.codex_auth_path.is_file() {
        return Ok(Vec::new());
    }
    let models = load_official_models()?;
    Ok(filter_selected_models(
        models,
        config.official_selected_models.as_deref(),
    ))
}

pub(super) fn filter_official_catalog(
    catalog: &mut Value,
    selected_models: Option<&[String]>,
) -> anyhow::Result<()> {
    let Some(selected_models) = selected_models else {
        return Ok(());
    };
    let selected_models = selected_models
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let models = catalog
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("official Codex catalog has no models array"))?;
    models.retain(|model| {
        model
            .get("slug")
            .and_then(Value::as_str)
            .is_some_and(|slug| selected_models.contains(slug))
    });
    Ok(())
}

pub(super) fn available_official_ids(models: &[ProviderModel]) -> HashSet<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

fn filter_selected_models(
    models: Vec<ProviderModel>,
    selected_models: Option<&[String]>,
) -> Vec<ProviderModel> {
    let Some(selected_models) = selected_models else {
        return models;
    };
    let selected_models = selected_models
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    models
        .into_iter()
        .filter(|model| selected_models.contains(model.id.as_str()))
        .collect()
}

fn official_models_from_catalog(catalog: &Value) -> anyhow::Result<Vec<ProviderModel>> {
    let models = catalog
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("official Codex catalog has no models array"))?;
    models
        .iter()
        .filter(|model| model.get("visibility").and_then(Value::as_str) != Some("hide"))
        .map(|model| {
            let id = model
                .get("slug")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("official Codex model is missing slug"))?;
            let supports_image = model
                .get("input_modalities")
                .and_then(Value::as_array)
                .is_some_and(|modalities| {
                    modalities
                        .iter()
                        .any(|modality| modality.as_str() == Some("image"))
                });
            let supports_thinking = model
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .is_some_and(|levels| !levels.is_empty());
            let supports_tool_search = model
                .get("experimental_supported_tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| {
                    tools
                        .iter()
                        .any(|tool| tool.as_str() == Some("tool_search"))
                });
            Ok(ProviderModel {
                id: id.to_owned(),
                display_name: model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                description: model
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                context_window: model
                    .get("context_window")
                    .and_then(Value::as_u64)
                    .or_else(|| model.get("max_context_window").and_then(Value::as_u64)),
                protocol: Some(ProviderProtocol::OpenAiResponses),
                api_path: Some("/responses".to_owned()),
                supports_image: Some(supports_image),
                supports_thinking: Some(supports_thinking),
                supports_web_search: Some(
                    model.get("supports_search_tool").and_then(Value::as_bool) == Some(true),
                ),
                supports_tool_search: Some(supports_tool_search),
                supports_function_tools: Some(true),
                ..ProviderModel::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::Router;
    use axum::routing::get;
    use codex_mixin::config::ThinkingMode;
    use serde_json::json;

    use super::*;

    #[test]
    fn converts_and_filters_official_catalog_models() {
        let catalog = json!({"models":[
            {
                "slug":"gpt-5.6-sol",
                "display_name":"GPT-5.6 Sol",
                "context_window":272000,
                "input_modalities":["text","image"],
                "supported_reasoning_levels":[{"effort":"high"}],
                "supports_search_tool":true
            },
            {"slug":"gpt-5.5","display_name":"GPT-5.5"},
            {"slug":"hidden-preview","visibility":"hide"}
        ]});
        let models = official_models_from_catalog(&catalog).unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].context_window, Some(272_000));
        assert_eq!(models[0].supports_image, Some(true));
        assert_eq!(models[0].supports_thinking, Some(true));
        assert_eq!(models[0].supports_web_search, Some(true));
        assert_eq!(
            filter_selected_models(models.clone(), None)
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-5.6-sol", "gpt-5.5"]
        );
        assert_eq!(
            filter_selected_models(models, Some(&["gpt-5.5".to_owned()]))
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-5.5"]
        );
    }

    #[test]
    fn explicit_empty_selection_removes_official_catalog_models() {
        let mut catalog = json!({"models":[{"slug":"gpt-5.5"}]});

        filter_official_catalog(&mut catalog, Some(&[])).unwrap();

        assert!(catalog["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn mixin_owned_catalog_takes_priority_over_codex_cache() {
        let directory = tempfile::tempdir().unwrap();
        let mixin_cache = directory.path().join("official-models.json");
        let codex_cache = directory.path().join("models_cache.json");
        std::fs::write(
            &mixin_cache,
            serde_json::to_vec(&json!({"models":[{"slug":"gpt-5.6-sol"}]})).unwrap(),
        )
        .unwrap();
        std::fs::write(
            &codex_cache,
            serde_json::to_vec(&json!({"models":[{"slug":"gpt-5.6-terra"}]})).unwrap(),
        )
        .unwrap();

        let models = load_official_models_from_paths(&mixin_cache, &codex_cache).unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.6-sol");
    }

    #[tokio::test]
    async fn live_refresh_persists_the_mixin_owned_catalog() {
        let upstream = Router::new().route(
            "/models",
            get(|| async { axum::Json(json!({"models":[{"slug":"gpt-5.6-sol"}]})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let auth_path = directory.path().join("auth.json");
        std::fs::write(
            &auth_path,
            r#"{"tokens":{"access_token":"secret","account_id":"account-one"}}"#,
        )
        .unwrap();
        let cache_path = directory.path().join("official-models.json");
        let config = GatewayConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            providers: Vec::new(),
            official_responses_url: format!("http://{address}/responses"),
            codex_auth_path: auth_path,
            gateway_api_key: None,
            accept_codex_oauth: true,
            official_selected_models: Some(vec!["gpt-5.6-terra".to_owned()]),
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(2),
            thinking_mode: ThinkingMode::Off,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles: Vec::new(),
        };

        let count = refresh_official_models_to_path(&config, "0.148.0", &cache_path)
            .await
            .unwrap();
        let models = load_official_models_from_paths(&cache_path, &cache_path).unwrap();

        assert_eq!(count, 1);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(config.official_selected_models.unwrap(), ["gpt-5.6-terra"]);
    }
}
