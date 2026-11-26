use std::collections::HashSet;
use std::path::Path;
use std::time::SystemTime;

use codex_mixin::catalog::load_template_catalog;
use codex_mixin::config::GatewayConfig;
use codex_mixin::provider::{ProviderModel, official_models_from_catalog};
use serde_json::Value;

use codex_mixin::catalog::official::{
    OFFICIAL_CATALOG_TIMEOUT, OFFICIAL_CATALOG_URL, fetch_catalog, validate_catalog,
};
use codex_mixin::catalog::official_models::{
    OFFICIAL_ACCOUNT_CATALOG_URL, catalog_for_current_account, is_official_account_catalog,
};
pub(in crate::cli) use codex_mixin::catalog::official_models::{
    apply_account_model_metadata, official_models_cache_path, official_template_cache_path,
    selected_official_models,
};
use codex_mixin::platform::write_atomic_if_changed;

pub(in crate::cli) const OFFICIAL_PROVIDER_ID: &str = "official";

pub(in crate::cli) fn load_official_models() -> anyhow::Result<Vec<ProviderModel>> {
    load_official_catalog()?.map_or_else(
        || Ok(Vec::new()),
        |catalog| official_models_from_catalog(&catalog),
    )
}
pub(in crate::cli) fn load_official_catalog() -> anyhow::Result<Option<Value>> {
    let auth_path = crate::cli::commands::connect::codex::codex_home_path().join("auth.json");
    codex_mixin::catalog::official_models::load_official_catalog(&auth_path)
}

pub(in crate::cli) fn load_official_account_catalog() -> anyhow::Result<Option<Value>> {
    let current = codex_mixin::provider::official_account_id(
        &crate::cli::commands::connect::codex::codex_home_path().join("auth.json"),
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

pub(in crate::cli) async fn refresh_official_models() -> anyhow::Result<usize> {
    let auth = crate::cli::commands::connect::codex::codex_home_path().join("auth.json");
    if codex_mixin::provider::official_auth_available(&auth)? {
        let mut catalog = codex_mixin::provider::fetch_official_resource(&auth, true).await?;
        catalog["codex_mixin_source"] = serde_json::json!(OFFICIAL_ACCOUNT_CATALOG_URL);
        return write_official_models_cache(&catalog, &official_models_cache_path());
    }
    refresh_official_models_to_path(OFFICIAL_CATALOG_URL, &official_models_cache_path()).await
}

pub(in crate::cli) fn same_account_catalog(previous: &Value, current: &Value) -> bool {
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

pub(in crate::cli) fn filter_official_catalog(
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

/// The single model list every coding client derives from: each enabled
/// provider's selected models that have cached metadata, then the official
/// models, then enabled routes, as `(catalog slug, display name, metadata)`.
/// The first use of a slug wins.
pub(in crate::cli) fn selected_catalog_models(
    config: &GatewayConfig,
    official_models: &[ProviderModel],
) -> Vec<(String, String, ProviderModel)> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for provider in config.providers.iter().filter(|provider| provider.enabled) {
        for upstream_model_id in &provider.selected_models {
            let Some(cached) = provider
                .cached_models
                .iter()
                .find(|candidate| &candidate.id == upstream_model_id)
            else {
                continue;
            };
            let id = codex_mixin::provider::catalog_model_slug(upstream_model_id, &provider.id);
            if seen.insert(id.clone()) {
                let name = codex_mixin::provider::provider_model_display_name(cached, provider);
                models.push((id, name, cached.clone()));
            }
        }
    }
    for model in official_models {
        if seen.insert(model.id.clone()) {
            let name = format!(
                "{} · OpenAI",
                model.display_name.as_deref().unwrap_or(&model.id)
            );
            models.push((model.id.clone(), name, model.clone()));
        }
    }
    for route in config.route_models() {
        if seen.insert(route.id.clone()) {
            models.push((route.id.clone(), route.id.clone(), route));
        }
    }
    models
}

pub(in crate::cli) fn available_official_ids(models: &[ProviderModel]) -> HashSet<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}
