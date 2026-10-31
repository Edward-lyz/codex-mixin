use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use codex_mixin::catalog::{apply_official_context_overrides, load_template_catalog};
use codex_mixin::config::{GatewayConfig, load_stored_config, stored_config_path};
use codex_mixin::provider::{ProviderModel, ProviderProtocol};
use serde_json::Value;

use codex_mixin::catalog::official::{
    OFFICIAL_CATALOG_TIMEOUT, OFFICIAL_CATALOG_URL, fetch_catalog, validate_catalog,
};
use codex_mixin::platform::write_atomic_if_changed;

pub(super) const OFFICIAL_PROVIDER_ID: &str = "official";
const OFFICIAL_ACCOUNT_CATALOG_URL: &str = "https://chatgpt.com/backend-api/codex/models";
const OFFICIAL_MODELS_CACHE_FILE: &str = "official-models.json";
const OFFICIAL_TEMPLATE_CACHE_FILE: &str = "official-template.json";

pub(super) fn load_official_models() -> anyhow::Result<Vec<ProviderModel>> {
    load_official_catalog()?.map_or_else(
        || Ok(Vec::new()),
        |catalog| official_models_from_catalog(&catalog),
    )
}

pub(super) fn load_official_catalog() -> anyhow::Result<Option<Value>> {
    let catalog = match load_template_catalog(Some(&official_models_cache_path()))? {
        Some(catalog) => Some(catalog),
        None => load_template_catalog(None)?,
    };
    catalog_for_current_account(catalog, || {
        codex_mixin::provider::official_account_id(
            &super::codex::codex_home_path().join("auth.json"),
        )
    })
}

fn catalog_for_current_account(
    catalog: Option<Value>,
    current_account: impl FnOnce() -> anyhow::Result<Option<String>>,
) -> anyhow::Result<Option<Value>> {
    if let Some(account) = catalog
        .as_ref()
        .and_then(|catalog| catalog.get("codex_mixin_account_id"))
        .and_then(Value::as_str)
        && current_account()?.as_deref() != Some(account)
    {
        return Ok(None);
    }
    Ok(catalog)
}

pub(super) fn load_official_account_catalog() -> anyhow::Result<Option<Value>> {
    let current = codex_mixin::provider::official_account_id(
        &super::codex::codex_home_path().join("auth.json"),
    )?;
    let Some(current) = current else {
        return Ok(None);
    };
    let catalog = load_template_catalog(Some(&official_models_cache_path()))?
        .filter(is_official_account_catalog);
    let catalog = catalog_for_current_account(catalog, || Ok(Some(current)))?;
    anyhow::ensure!(
        catalog.is_some(),
        "current official account model catalog is missing; refresh the OpenAI model list first"
    );
    Ok(catalog)
}

pub(super) fn is_official_account_catalog(catalog: &Value) -> bool {
    catalog.get("codex_mixin_source").and_then(Value::as_str) == Some(OFFICIAL_ACCOUNT_CATALOG_URL)
        && catalog
            .get("codex_mixin_account_id")
            .and_then(Value::as_str)
            .is_some_and(|account| !account.is_empty())
}

pub(super) async fn refresh_official_models() -> anyhow::Result<usize> {
    let auth = super::codex::codex_home_path().join("auth.json");
    if codex_mixin::provider::official_auth_available(&auth)? {
        let mut catalog = codex_mixin::provider::fetch_official_resource(&auth, true).await?;
        catalog["codex_mixin_source"] = serde_json::json!(OFFICIAL_ACCOUNT_CATALOG_URL);
        return write_official_models_cache(&catalog, &official_models_cache_path());
    }
    refresh_official_models_to_path(OFFICIAL_CATALOG_URL, &official_models_cache_path()).await
}

pub(in crate::cli) fn official_models_cache_path() -> PathBuf {
    stored_config_path().with_file_name(OFFICIAL_MODELS_CACHE_FILE)
}

pub(in crate::cli) fn official_template_cache_path() -> PathBuf {
    stored_config_path().with_file_name(OFFICIAL_TEMPLATE_CACHE_FILE)
}

pub(super) fn same_account_catalog(previous: &Value, current: &Value) -> bool {
    previous.get("codex_mixin_source") == current.get("codex_mixin_source")
        && previous
            .get("codex_mixin_source")
            .and_then(Value::as_str)
            .is_some()
        && previous
            .get("codex_mixin_account_id")
            .and_then(Value::as_str)
            .is_some_and(|account| !account.is_empty())
        && previous.get("codex_mixin_account_id") == current.get("codex_mixin_account_id")
}

pub(in crate::cli) fn write_official_models_cache(
    catalog: &Value,
    cache_path: &Path,
) -> anyhow::Result<usize> {
    validate_catalog(catalog)?;
    let model_count = official_models_from_catalog(catalog)?.len();
    anyhow::ensure!(
        model_count > 0,
        "Codex repository catalog returned no visible models"
    );
    let mut cached = catalog.clone();
    if cached.get("codex_mixin_source").is_none() {
        cached["codex_mixin_source"] = serde_json::json!(OFFICIAL_CATALOG_URL);
    }
    write_atomic_if_changed(cache_path, &serde_json::to_vec_pretty(&cached)?)?;
    // A successful refresh can return the same catalog. Keep the timestamp
    // meaningful for the provider UI instead of showing only content changes.
    std::fs::OpenOptions::new()
        .write(true)
        .open(cache_path)?
        .set_modified(SystemTime::now())?;
    Ok(model_count)
}

async fn refresh_official_models_to_path(
    source_url: &str,
    cache_path: &Path,
) -> anyhow::Result<usize> {
    let cached = load_template_catalog(Some(cache_path))?;
    let catalog = fetch_catalog(source_url, OFFICIAL_CATALOG_TIMEOUT, cached.as_ref()).await?;
    write_official_models_cache(&catalog, cache_path)
}

// Existing client exports may be prepared before OAuth is completed; requests still validate credentials.
pub(super) fn selected_official_models(
    config: &GatewayConfig,
) -> anyhow::Result<Vec<ProviderModel>> {
    if !config.accept_codex_oauth
        || !(config.codex_auth_path.is_file()
            || codex_mixin::provider::official_auth_available(&config.codex_auth_path)?)
    {
        return Ok(Vec::new());
    }
    let Some(mut catalog) = load_official_catalog()? else {
        return Ok(Vec::new());
    };
    let stored = load_stored_config()?
        .ok_or_else(|| anyhow::anyhow!("provider configuration is missing"))?;
    apply_official_context_overrides(&mut catalog, &stored.official_model_contexts)?;
    let models = official_models_from_catalog(&catalog)?;
    Ok(filter_selected_models(
        models,
        config.official_selected_models.as_deref(),
    ))
}

pub(super) fn filter_official_catalog(
    catalog: &mut Value,
    selected_models: Option<&[String]>,
) -> anyhow::Result<()> {
    let models = catalog
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("official Codex catalog has no models array"))?;
    if let Some(selected_models) = selected_models {
        let positions = selected_models
            .iter()
            .enumerate()
            .map(|(index, model)| (model.as_str(), index))
            .collect::<std::collections::HashMap<_, _>>();
        models.retain(|model| {
            model
                .get("slug")
                .and_then(Value::as_str)
                .is_some_and(|slug| positions.contains_key(slug))
        });
        models.sort_by_key(|model| {
            model
                .get("slug")
                .and_then(Value::as_str)
                .and_then(|slug| positions.get(slug))
                .copied()
        });
    } else {
        models.sort_by(|left, right| {
            codex_mixin::provider::compare_model_ids(
                left["slug"].as_str().unwrap_or_default(),
                right["slug"].as_str().unwrap_or_default(),
            )
        });
    }
    for (index, model) in models.iter_mut().enumerate() {
        model["priority"] = serde_json::json!(index);
    }
    Ok(())
}

pub(super) fn available_official_ids(models: &[ProviderModel]) -> HashSet<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

fn filter_selected_models(
    mut models: Vec<ProviderModel>,
    selected_models: Option<&[String]>,
) -> Vec<ProviderModel> {
    let Some(selected_models) = selected_models else {
        models.sort_by(|left, right| codex_mixin::provider::compare_model_ids(&left.id, &right.id));
        return models;
    };
    let positions = selected_models
        .iter()
        .enumerate()
        .map(|(index, model)| (model.as_str(), index))
        .collect::<std::collections::HashMap<_, _>>();
    models.retain(|model| positions.contains_key(model.id.as_str()));
    models.sort_by_key(|model| positions.get(model.id.as_str()).copied());
    models
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
