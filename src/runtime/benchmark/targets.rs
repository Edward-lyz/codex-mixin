//! Benchmark target resolution: which `(provider, model)` pairs a benchmark run
//! may measure.
//!
//! This lives in the benchmark layer so the catalog never has to depend on
//! benchmarking. It uses `catalog` and `provider` public APIs, both of which sit
//! below this module.

use std::sync::Arc;

use serde_json::Value;

use crate::catalog::load_template_catalog;
use crate::config::{GatewayConfig, stored_config_path};
use crate::error::GatewayError;
use crate::provider::{ProviderRegistry, catalog_model_slug};

use super::BenchmarkTarget;

/// Resolve the benchmark targets for the requested provider and model filters.
///
/// Every entry is validated before it is returned: an unknown provider, an
/// unroutable model, or an official request without an enabled OAuth account
/// fails the whole request rather than silently benchmarking a subset.
pub(crate) async fn resolve_targets(
    config: &Arc<GatewayConfig>,
    providers: &ProviderRegistry,
    provider_filters: &[String],
    model_filters: &[String],
) -> Result<Vec<BenchmarkTarget>, GatewayError> {
    let official_models = official_benchmark_models(config, provider_filters).await?;
    validate_provider_filters(provider_filters, providers, &official_models)?;
    validate_model_filters(model_filters, providers, &official_models)?;

    let mut targets = Vec::new();
    for provider in providers.providers() {
        if !provider_filters.is_empty()
            && !provider_filters
                .iter()
                .any(|provider_id| provider_id == provider.id())
        {
            continue;
        }
        for upstream_model_id in &provider.definition().selected_models {
            let catalog_slug = catalog_model_slug(upstream_model_id, provider.id());
            let Some(resolved) = providers.resolve(&catalog_slug) else {
                continue;
            };
            if !model_filters.is_empty()
                && !model_filters.iter().any(|model| model == &catalog_slug)
            {
                continue;
            }
            targets.push(BenchmarkTarget {
                catalog_slug,
                provider_id: provider.id().to_owned(),
                provider_name: provider.display_name().to_owned(),
                upstream_model_id: resolved.upstream_model_id.to_owned(),
                provider: Some(provider.clone()),
            });
        }
    }
    for model in official_models {
        if !model_filters.is_empty() && !model_filters.contains(&model) {
            continue;
        }
        targets.push(BenchmarkTarget {
            catalog_slug: model.clone(),
            provider_id: "official".to_owned(),
            provider_name: "OpenAI".to_owned(),
            upstream_model_id: model,
            provider: None,
        });
    }
    Ok(targets)
}

/// Official (ChatGPT OAuth) models the caller may benchmark, read from the
/// cached official catalog and checked against the signed-in account.
async fn official_benchmark_models(
    config: &Arc<GatewayConfig>,
    provider_filters: &[String],
) -> Result<Vec<String>, GatewayError> {
    let official_requested =
        provider_filters.is_empty() || provider_filters.iter().any(|id| id == "official");
    if !official_requested || !config.accept_codex_oauth {
        return Ok(Vec::new());
    }
    let config = Arc::clone(config);
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<String>> {
        if !crate::provider::official_auth_available(&config.codex_auth_path)? {
            return Ok(Vec::new());
        }
        let cache = stored_config_path().with_file_name("official-models.json");
        let catalog = match load_template_catalog(Some(&cache))? {
            Some(catalog) => Some(catalog),
            None => load_template_catalog(None)?,
        };
        if let Some(account) = catalog
            .as_ref()
            .and_then(|value| value.get("codex_mixin_account_id"))
            .and_then(Value::as_str)
            && crate::provider::official_account_id(&config.codex_auth_path)?.as_deref()
                != Some(account)
        {
            return Ok(Vec::new());
        }
        let available = catalog
            .as_ref()
            .and_then(|value| value.get("models"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("slug").and_then(Value::as_str))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        match &config.official_selected_models {
            Some(selected) => Ok(selected
                .iter()
                .filter(|model| available.contains(model))
                .cloned()
                .collect()),
            None => Ok(available),
        }
    })
    .await
    .map_err(|error| anyhow::anyhow!("official catalog loader failed: {error}"))?
    .map_err(GatewayError::Other)
}

fn validate_provider_filters(
    provider_filters: &[String],
    providers: &ProviderRegistry,
    official_models: &[String],
) -> Result<(), GatewayError> {
    for provider_id in provider_filters {
        if provider_id != "official" && providers.provider(provider_id).is_none() {
            return Err(GatewayError::BadRequest(format!(
                "unknown benchmark provider: {provider_id}"
            )));
        }
        if provider_id == "official" && official_models.is_empty() {
            return Err(GatewayError::BadRequest(
                "official benchmark requires an enabled OAuth account and selected models"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_model_filters(
    model_filters: &[String],
    providers: &ProviderRegistry,
    official_models: &[String],
) -> Result<(), GatewayError> {
    for model in model_filters {
        if providers.resolve(model).is_none() && !official_models.contains(model) {
            return Err(GatewayError::BadRequest(format!(
                "benchmark model is not routable: {model}"
            )));
        }
    }
    Ok(())
}
