use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use codex_mixin::anthropic::ModelInfo;
use codex_mixin::catalog::{
    apply_auto_review_override, apply_official_context_overrides, apply_web_search_capabilities,
    codex_catalog_from_models_with_metadata, codex_oauth_proxy_catalog, load_template_catalog,
    migrate_managed_model_metadata, refresh_managed_oauth_catalog,
};
use codex_mixin::config::{GatewayConfig, load_stored_config};
use codex_mixin::provider::{MetadataResolver, auxiliary_auto_review_slug};
use codex_mixin::server::AppState;
use codex_mixin::web_search::WebSearchCapabilities;

use super::managed_config::*;
use crate::cli::metadata::load_model_metadata_resolver;
use crate::cli::official_models::{
    filter_official_catalog, is_official_account_catalog, load_official_account_catalog,
    load_official_catalog, official_template_cache_path, write_official_models_cache,
};
use codex_mixin::catalog::official::{
    OFFICIAL_CATALOG_TIMEOUT, OFFICIAL_CATALOG_URL, fetch_catalog,
};
use codex_mixin::platform::write_atomic_if_changed;

pub(in crate::cli) async fn refresh_default_managed_codex_catalog() -> anyhow::Result<()> {
    let config_path = resolve_codex_config_path(None)?;
    let Some((oauth_proxy, catalog_path)) = managed_catalog_settings(&config_path)? else {
        println!("Codex model catalog is not managed by codex-mixin");
        return Ok(());
    };
    let gateway_config = GatewayConfig::from_stored_config()?;
    let state = AppState::new(gateway_config.clone())?;
    // An explicit empty selection must clear the managed catalog too.
    let models = state.fetch_models().await?;
    let mut template = if oauth_proxy {
        let account_catalog = tokio::task::spawn_blocking(load_official_account_catalog).await??;
        load_official_catalog_for_sync(
            OFFICIAL_CATALOG_URL,
            &official_template_cache_path(),
            account_catalog.as_ref(),
        )
        .await?
    } else {
        None
    };
    if let Some(template) = &mut template {
        filter_official_catalog(template, gateway_config.official_selected_models.as_deref())?;
    }
    let metadata = load_model_metadata_resolver().await?;
    let catalog = generated_codex_catalog(
        &models,
        gateway_config.default_context_window,
        template.as_ref(),
        &metadata,
        oauth_proxy,
    );
    let supported_models =
        WebSearchCapabilities::from_default_path(&gateway_config)?.supported_model_ids();
    let auto_review_slug = auxiliary_auto_review_slug(&gateway_config.providers);
    let stored_config = load_stored_config()?
        .ok_or_else(|| anyhow::anyhow!("provider configuration is missing"))?;
    if write_generated_managed_codex_catalog(
        &config_path,
        catalog,
        &supported_models,
        auto_review_slug.as_deref(),
        oauth_proxy.then_some(&stored_config.official_model_contexts),
    )? {
        println!("Codex model catalog refreshed: {}", catalog_path.display());
    } else {
        println!(
            "Codex model catalog already current: {}",
            catalog_path.display()
        );
    }
    if let Some(summary) = managed_catalog_summary(&config_path)? {
        println!("catalog mode: {}", summary.mode);
        println!("catalog models: {}", summary.model_count);
        println!("catalog managed models: {}", summary.managed_model_count);
    }
    Ok(())
}

pub(in crate::cli) fn managed_catalog_settings(
    config_path: &Path,
) -> anyhow::Result<Option<(bool, PathBuf)>> {
    let config_path = absolute_path(config_path.to_path_buf())?;
    if !config_path.exists() {
        return Ok(None);
    }
    let raw_config = fs::read_to_string(&config_path)?;
    if !codex_mixin::clients::codex::document_is_managed(&raw_config) {
        return Ok(None);
    }
    let doc = raw_config.parse::<DocumentMut>()?;
    let provider_id = codex_mixin::clients::codex::managed_provider_id(&doc)?;
    let provider = doc
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table)
        .ok_or_else(|| {
            anyhow::anyhow!("managed Codex config has no {provider_id} provider table")
        })?;
    let oauth_proxy = managed_oauth_proxy_mode(provider_id, provider)?;
    let catalog_path = managed_catalog_path(&doc, &config_path)?;
    Ok(Some((oauth_proxy, catalog_path)))
}

/// Build the managed Codex catalog for the selected install mode.
pub(in crate::cli) fn generated_codex_catalog(
    models: &[ModelInfo],
    default_context_window: u64,
    template: Option<&serde_json::Value>,
    metadata: &MetadataResolver,
    oauth_proxy: bool,
) -> serde_json::Value {
    if oauth_proxy {
        codex_oauth_proxy_catalog(models, default_context_window, template, metadata, None)
    } else {
        codex_catalog_from_models_with_metadata(models, default_context_window, template, metadata)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::cli) struct ManagedCatalogSummary {
    pub catalog_path: PathBuf,
    pub mode: &'static str,
    pub model_count: usize,
    pub managed_model_count: usize,
}

pub(in crate::cli) fn managed_catalog_summary(
    config_path: &Path,
) -> anyhow::Result<Option<ManagedCatalogSummary>> {
    let Some((oauth_proxy, catalog_path)) = managed_catalog_settings(config_path)? else {
        return Ok(None);
    };
    let catalog: serde_json::Value = serde_json::from_slice(&fs::read(&catalog_path)?)?;
    let models = catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("managed Codex model catalog has no models array"))?;
    Ok(Some(ManagedCatalogSummary {
        catalog_path,
        mode: codex_install_mode_name(oauth_proxy),
        model_count: models.len(),
        managed_model_count: models
            .iter()
            .filter(|model| is_managed_catalog_model(model))
            .count(),
    }))
}

pub(in crate::cli) fn managed_codex_install_mode(
    config_path: &Path,
) -> anyhow::Result<Option<&'static str>> {
    Ok(managed_catalog_settings(config_path)?
        .map(|(oauth_proxy, _)| codex_install_mode_name(oauth_proxy)))
}

fn codex_install_mode_name(oauth_proxy: bool) -> &'static str {
    if oauth_proxy {
        "codex_oauth_proxy"
    } else {
        "custom_only"
    }
}

pub(in crate::cli) fn write_generated_managed_codex_catalog(
    config_path: &Path,
    mut catalog: serde_json::Value,
    supported_web_search_models: &HashSet<String>,
    auto_review_slug: Option<&str>,
    official_contexts: Option<&BTreeMap<String, u64>>,
) -> anyhow::Result<bool> {
    let config_path = absolute_path(config_path.to_path_buf())?;
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
    let raw_config = fs::read_to_string(&config_path)?;
    if !codex_mixin::clients::codex::document_is_managed(&raw_config) {
        return Ok(false);
    }
    let mut doc = raw_config.parse::<DocumentMut>()?;
    let catalog_path = managed_catalog_path(&doc, &config_path)?;
    apply_web_search_capabilities(&mut catalog, supported_web_search_models)?;
    apply_auto_review_override(&mut catalog, auto_review_slug)?;
    if let Some(official_contexts) = official_contexts {
        apply_official_context_overrides(&mut catalog, official_contexts)?;
    }
    let config_changed = if remove_unavailable_default_model(&mut doc, &catalog) {
        write_atomic_if_changed(&config_path, doc.to_string().as_bytes())?
    } else {
        false
    };
    let catalog_changed =
        write_atomic_if_changed(&catalog_path, &serde_json::to_vec_pretty(&catalog)?)?;
    Ok(config_changed || catalog_changed)
}

pub(in crate::cli) fn refresh_managed_codex_catalog_with_capabilities(
    config_path: &Path,
    supported_web_search_models: Option<&HashSet<String>>,
    auto_review_slug: Option<&str>,
) -> anyhow::Result<bool> {
    refresh_managed_codex_catalog_with_source(
        config_path,
        None,
        supported_web_search_models,
        auto_review_slug,
        None,
    )
}

pub(in crate::cli) fn refresh_managed_codex_catalog_from_official(
    config_path: &Path,
    official_catalog: &serde_json::Value,
    supported_web_search_models: Option<&HashSet<String>>,
    auto_review_slug: Option<&str>,
    official_contexts: &BTreeMap<String, u64>,
) -> anyhow::Result<bool> {
    refresh_managed_codex_catalog_with_source(
        config_path,
        Some(official_catalog),
        supported_web_search_models,
        auto_review_slug,
        Some(official_contexts),
    )
}

fn refresh_managed_codex_catalog_with_source(
    config_path: &Path,
    official_catalog: Option<&serde_json::Value>,
    supported_web_search_models: Option<&HashSet<String>>,
    auto_review_slug: Option<&str>,
    official_contexts: Option<&BTreeMap<String, u64>>,
) -> anyhow::Result<bool> {
    let config_path = absolute_path(config_path.to_path_buf())?;
    if !config_path.exists() {
        return Ok(false);
    }
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
    if !config_path.exists() {
        return Ok(false);
    }
    let raw_config = fs::read_to_string(&config_path)?;
    if !codex_mixin::clients::codex::document_is_managed(&raw_config) {
        return Ok(false);
    }
    let mut doc = raw_config.parse::<DocumentMut>()?;
    let provider_id = codex_mixin::clients::codex::managed_provider_id(&doc)?;
    let provider = doc
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table)
        .ok_or_else(|| {
            anyhow::anyhow!("managed Codex config has no {provider_id} provider table")
        })?;
    let oauth_proxy = managed_oauth_proxy_mode(provider_id, provider)?;
    if !oauth_proxy && supported_web_search_models.is_none() {
        return Ok(false);
    }
    let catalog_path = managed_catalog_path(&doc, &config_path)?;
    let mut managed_catalog = serde_json::from_slice(&fs::read(&catalog_path)?)?;
    migrate_managed_model_metadata(&mut managed_catalog)?;
    let mut refreshed = if oauth_proxy && let Some(official_catalog) = official_catalog {
        refresh_managed_oauth_catalog(official_catalog, &managed_catalog)?
    } else {
        managed_catalog
    };
    if let Some(supported_web_search_models) = supported_web_search_models {
        apply_web_search_capabilities(&mut refreshed, supported_web_search_models)?;
    }
    apply_auto_review_override(&mut refreshed, auto_review_slug)?;
    if let Some(official_contexts) = official_contexts {
        apply_official_context_overrides(&mut refreshed, official_contexts)?;
    }
    let config_changed = if remove_unavailable_default_model(&mut doc, &refreshed) {
        write_atomic_if_changed(&config_path, doc.to_string().as_bytes())?
    } else {
        false
    };
    let catalog_changed =
        write_atomic_if_changed(&catalog_path, &serde_json::to_vec_pretty(&refreshed)?)?;
    Ok(config_changed || catalog_changed)
}

fn remove_unavailable_default_model(doc: &mut DocumentMut, catalog: &serde_json::Value) -> bool {
    let Some(current_model) = doc.get("model").and_then(Item::as_str) else {
        return false;
    };
    if template_catalog_has_model(Some(catalog), current_model) {
        return false;
    }
    doc.remove("model");
    true
}

fn managed_oauth_proxy_mode(
    provider_id: &str,
    provider: &toml_edit::Table,
) -> anyhow::Result<bool> {
    if provider_id == codex_mixin::clients::codex::CUSTOM_ONLY_PROVIDER {
        return Ok(false);
    }
    match provider.get("supports_websockets") {
        Some(item) => item
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("codex-mixin supports_websockets must be a boolean")),
        None => Ok(false),
    }
}

pub(in crate::cli) async fn load_codex_install_template_online(
    codex_oauth_proxy: bool,
    official_models_cache: &Path,
    source_url: &str,
    local_catalog: impl FnOnce() -> anyhow::Result<Option<serde_json::Value>>,
    account_catalog: Option<&serde_json::Value>,
) -> anyhow::Result<Option<serde_json::Value>> {
    if !codex_oauth_proxy {
        return Ok(None);
    }
    load_preferred_official_catalog(
        source_url,
        official_models_cache,
        local_catalog,
        account_catalog,
    )
    .await
}

pub(in crate::cli) async fn refresh_managed_official_codex_catalog(
    config_path: &Path,
    state: &AppState,
    supported_web_search_models: Option<&HashSet<String>>,
) -> anyhow::Result<bool> {
    let Some((oauth_proxy, _)) = managed_catalog_settings(config_path)? else {
        return Ok(false);
    };
    if !oauth_proxy {
        return Ok(false);
    }
    let mut official_catalog = state
        .fetch_official_models_catalog(OFFICIAL_CATALOG_URL)
        .await?;
    let official_cache = official_template_cache_path();
    write_official_models_cache(&official_catalog, &official_cache)?;
    if let Some(account_catalog) =
        tokio::task::spawn_blocking(load_official_account_catalog).await??
    {
        apply_account_model_metadata(&mut official_catalog, &account_catalog)?;
    }
    let gateway_config = GatewayConfig::from_stored_config()?;
    filter_official_catalog(
        &mut official_catalog,
        gateway_config.official_selected_models.as_deref(),
    )?;
    let auto_review_slug = auxiliary_auto_review_slug(&gateway_config.providers);
    let stored_config = load_stored_config()?
        .ok_or_else(|| anyhow::anyhow!("provider configuration is missing"))?;
    refresh_managed_codex_catalog_from_official(
        config_path,
        &official_catalog,
        supported_web_search_models,
        auto_review_slug.as_deref(),
        &stored_config.official_model_contexts,
    )
}

// Selection/apply only regenerates the local catalog. The running gateway
// refreshes the public template separately; do not download it twice per save.
async fn load_official_catalog_for_sync(
    source_url: &str,
    official_cache: &Path,
    account_catalog: Option<&serde_json::Value>,
) -> anyhow::Result<Option<serde_json::Value>> {
    if let Some(mut cached) = load_template_catalog(Some(official_cache))? {
        codex_mixin::catalog::official::validate_catalog(&cached)?;
        if let Some(account_catalog) = account_catalog {
            apply_account_model_metadata(&mut cached, account_catalog)?;
        }
        return Ok(Some(cached));
    }
    load_preferred_official_catalog(
        source_url,
        official_cache,
        load_official_catalog,
        account_catalog,
    )
    .await
}

async fn load_preferred_official_catalog(
    source_url: &str,
    official_cache: &Path,
    // Legacy fallback stays lazy: an unrelated native cache must not block a
    // successful public template refresh. The account overlay is loaded separately.
    local_catalog: impl FnOnce() -> anyhow::Result<Option<serde_json::Value>>,
    account_catalog: Option<&serde_json::Value>,
) -> anyhow::Result<Option<serde_json::Value>> {
    let cached = load_template_catalog(Some(official_cache))?;
    let mut template = match fetch_catalog(source_url, OFFICIAL_CATALOG_TIMEOUT, cached.as_ref())
        .await
    {
        Ok(catalog) => {
            write_official_models_cache(&catalog, official_cache)?;
            Some(catalog)
        }
        Err(error) => {
            let fallback = match load_template_catalog(Some(official_cache))? {
                Some(cached) => Some(cached),
                None => local_catalog()?.or_else(|| account_catalog.cloned()),
            };
            let Some(cached) = fallback else {
                eprintln!(
                    "warning: official model template unavailable; continuing with configured provider models: {error:#}"
                );
                return Ok(None);
            };
            codex_mixin::catalog::official::validate_catalog(&cached)?;
            tracing::warn!(event = "official_catalog_stale", source_url, error = %error,
                cache = %official_cache.display(), "public catalog refresh failed; retaining last known Mixin cache");
            Some(cached)
        }
    };
    if let (Some(template), Some(account_catalog)) = (&mut template, account_catalog) {
        apply_account_model_metadata(template, account_catalog)?;
    }
    Ok(template)
}

fn apply_account_model_metadata(
    template: &mut serde_json::Value,
    account_catalog: &serde_json::Value,
) -> anyhow::Result<()> {
    if !is_official_account_catalog(account_catalog) {
        return Ok(());
    }
    codex_mixin::catalog::official::validate_catalog(account_catalog)?;
    let account_models = account_catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("official account catalog has no models array"))?;
    anyhow::ensure!(
        !account_models.iter().any(is_managed_catalog_model),
        "official account catalog contains managed provider models"
    );
    let template_models = template
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("official template has no models array"))?;
    let by_slug = template_models
        .iter()
        .filter_map(|model| Some((model.get("slug")?.as_str()?, model)))
        .collect::<HashMap<_, _>>();
    let mut merged = Vec::with_capacity(account_models.len());
    for account_model in account_models {
        let slug = account_model
            .get("slug")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("official account model is missing slug"))?;
        // A legacy mixed template may also contain provider-owned models.
        // Account metadata must never replace their identity or capabilities.
        if by_slug
            .get(slug)
            .is_some_and(|model| is_managed_catalog_model(model))
        {
            continue;
        }
        let mut model = by_slug
            .get(slug)
            .map_or_else(|| account_model.clone(), |model| (*model).clone());
        let fields = model
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("official template model must be an object"))?;
        // An account may lose a tier or explicitly advertise none.
        for field in [
            "service_tiers",
            "additional_speed_tiers",
            "visibility",
            "supported_in_api",
            "priority",
        ] {
            match account_model.get(field) {
                Some(value) => {
                    fields.insert(field.to_owned(), value.clone());
                }
                None => {
                    fields.remove(field);
                }
            }
        }
        // Missing context fields retain the template value; explicit nulls
        // clear optional limits. Instructions and tools stay with the template.
        for field in [
            "context_window",
            "max_context_window",
            "auto_compact_token_limit",
        ] {
            if let Some(value) = account_model.get(field) {
                anyhow::ensure!(
                    value.is_null() || value.as_i64().is_some_and(|limit| limit > 0),
                    "official account model {field} must be null or a positive 64-bit integer"
                );
                fields.insert(field.to_owned(), value.clone());
            }
        }
        if let (Some(window), Some(maximum)) = (
            fields
                .get("context_window")
                .and_then(serde_json::Value::as_i64),
            fields
                .get("max_context_window")
                .and_then(serde_json::Value::as_i64),
        ) {
            anyhow::ensure!(
                window <= maximum,
                "official account context_window exceeds max_context_window"
            );
        }
        merged.push(model);
    }
    merged.extend(
        template_models
            .iter()
            .filter(|model| is_managed_catalog_model(model))
            .cloned(),
    );
    template["models"] = serde_json::Value::Array(merged);
    Ok(())
}

pub(in crate::cli) fn is_managed_catalog_model(model: &serde_json::Value) -> bool {
    model
        .get("codex_mixin_managed")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        || model
            .get("description")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|description| {
                description.starts_with("Custom upstream model exposed through codex-")
            })
        || model
            .get("slug")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|slug| slug.ends_with("-custom"))
}

pub(in crate::cli) fn select_codex_model(
    requested_model: Option<String>,
    models: &[codex_mixin::anthropic::ModelInfo],
    doc: &DocumentMut,
) -> anyhow::Result<String> {
    if let Some(model) = requested_model {
        if models.iter().any(|candidate| candidate.id == model) {
            return Ok(model);
        }
        anyhow::bail!("requested model is not present in upstream /v1/models: {model}");
    }
    if let Some(current_model) = doc.get("model").and_then(Item::as_str)
        && models.iter().any(|candidate| candidate.id == current_model)
    {
        return Ok(current_model.to_owned());
    }
    if let Some(model) = models.iter().find(|model| model.id == "Claude Sonnet 5") {
        return Ok(model.id.clone());
    }
    Ok(models[0].id.clone())
}

pub(in crate::cli) fn select_codex_oauth_proxy_model(
    requested_model: Option<String>,
    models: &[codex_mixin::anthropic::ModelInfo],
    template_catalog: Option<&serde_json::Value>,
    doc: &DocumentMut,
) -> anyhow::Result<String> {
    if let Some(model) = requested_model {
        if model_exists_in_oauth_proxy_catalog(&model, models, template_catalog) {
            return Ok(model);
        }
        anyhow::bail!("requested model is not present in generated Codex catalog: {model}");
    }
    if let Some(current_model) = doc.get("model").and_then(Item::as_str)
        && model_exists_in_oauth_proxy_catalog(current_model, models, template_catalog)
    {
        return Ok(current_model.to_owned());
    }
    for preferred in ["gpt-5.5", "gpt-5.4", "gpt-5.4-mini"] {
        if template_catalog_has_model(template_catalog, preferred) {
            return Ok(preferred.to_owned());
        }
    }
    if let Some(model) = template_catalog
        .and_then(|catalog| catalog.get("models"))
        .and_then(serde_json::Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
                .find(|slug| is_gpt_model(slug))
        })
    {
        return Ok(model.to_owned());
    }
    Ok(models[0].id.clone())
}

pub(in crate::cli) fn model_exists_in_oauth_proxy_catalog(
    model: &str,
    models: &[codex_mixin::anthropic::ModelInfo],
    template_catalog: Option<&serde_json::Value>,
) -> bool {
    if template_catalog_has_model(template_catalog, model) {
        return true;
    }
    models.iter().any(|candidate| candidate.id == model)
}

pub(in crate::cli) fn template_catalog_has_model(
    template_catalog: Option<&serde_json::Value>,
    slug: &str,
) -> bool {
    template_catalog
        .and_then(|catalog| catalog.get("models"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|models| {
            models
                .iter()
                .any(|model| model.get("slug").and_then(serde_json::Value::as_str) == Some(slug))
        })
}

pub(in crate::cli) fn is_gpt_model(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("gpt-")
}
