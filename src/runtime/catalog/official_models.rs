use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{GatewayConfig, load_stored_config_read_only, stored_config_path};
use crate::provider::{
    ProviderModel, compare_model_ids, official_account_id, official_auth_available,
    official_models_from_catalog,
};

use super::{apply_official_context_overrides, load_template_catalog};

pub const OFFICIAL_ACCOUNT_CATALOG_URL: &str = "https://chatgpt.com/backend-api/codex/models";
const OFFICIAL_MODELS_CACHE_FILE: &str = "official-models.json";
const OFFICIAL_TEMPLATE_CACHE_FILE: &str = "official-template.json";

pub fn official_models_cache_path() -> PathBuf {
    stored_config_path().with_file_name(OFFICIAL_MODELS_CACHE_FILE)
}

pub fn official_template_cache_path() -> PathBuf {
    stored_config_path().with_file_name(OFFICIAL_TEMPLATE_CACHE_FILE)
}

pub fn load_official_catalog(auth_path: &Path) -> anyhow::Result<Option<Value>> {
    let catalog = match load_template_catalog(Some(&official_models_cache_path()))? {
        Some(catalog) => Some(catalog),
        None => load_template_catalog(None)?,
    };
    let account_catalog = catalog_for_current_account(catalog, || official_account_id(auth_path))?;
    if let Some(account_catalog) = account_catalog.as_ref()
        && is_official_account_catalog(account_catalog)
        && let Some(mut template) = load_template_catalog(Some(&official_template_cache_path()))?
    {
        apply_account_model_metadata(&mut template, account_catalog)?;
        template["codex_mixin_source"] = account_catalog["codex_mixin_source"].clone();
        template["codex_mixin_account_id"] = account_catalog["codex_mixin_account_id"].clone();
        return Ok(Some(template));
    }
    Ok(account_catalog)
}

pub fn catalog_for_current_account(
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

pub fn is_official_account_catalog(catalog: &Value) -> bool {
    catalog.get("codex_mixin_source").and_then(Value::as_str) == Some(OFFICIAL_ACCOUNT_CATALOG_URL)
        && catalog
            .get("codex_mixin_account_id")
            .and_then(Value::as_str)
            .is_some_and(|account| !account.is_empty())
}

/// Return the selected cached models for the authenticated official account.
pub fn selected_official_models(config: &GatewayConfig) -> anyhow::Result<Vec<ProviderModel>> {
    if !config.accept_codex_oauth || !official_auth_available(&config.codex_auth_path)? {
        return Ok(Vec::new());
    }
    let Some(mut catalog) = load_official_catalog(&config.codex_auth_path)? else {
        return Ok(Vec::new());
    };
    if let Some(stored) = load_stored_config_read_only()? {
        apply_official_context_overrides(&mut catalog, &stored.official_model_contexts)?;
    }
    let models = official_models_from_catalog(&catalog)?;
    Ok(filter_selected_models(
        models,
        config.official_selected_models.as_deref(),
    ))
}

fn filter_selected_models(
    mut models: Vec<ProviderModel>,
    selected_models: Option<&[String]>,
) -> Vec<ProviderModel> {
    let Some(selected_models) = selected_models else {
        models.sort_by(|left, right| compare_model_ids(&left.id, &right.id));
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

pub fn apply_account_model_metadata(
    template: &mut serde_json::Value,
    account_catalog: &serde_json::Value,
) -> anyhow::Result<()> {
    if !is_official_account_catalog(account_catalog) {
        return Ok(());
    }
    super::official::validate_catalog(account_catalog)?;
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
            .filter(|model| {
                is_managed_catalog_model(model)
                    || !account_models
                        .iter()
                        .any(|account| account["slug"] == model["slug"])
            })
            .cloned(),
    );
    template["models"] = serde_json::Value::Array(merged);
    Ok(())
}

pub fn is_managed_catalog_model(model: &serde_json::Value) -> bool {
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
