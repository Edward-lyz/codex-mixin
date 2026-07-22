use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use codex_mixin::CODEX_MIXIN_PROVIDER;
use codex_mixin::catalog::{
    apply_web_search_capabilities, codex_catalog_from_models_with_metadata,
    codex_oauth_proxy_catalog_from_models_with_metadata_for_provider, load_template_catalog,
    refresh_managed_oauth_catalog,
};
use codex_mixin::config::{GatewayConfig, ProviderPreset};
use codex_mixin::server::AppState;
use codex_mixin::web_search::WebSearchCapabilities;

use super::managed_config::*;
use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::metadata::load_model_metadata_resolver;

pub(in crate::cli) async fn refresh_default_managed_codex_catalog() -> anyhow::Result<()> {
    let config_path = resolve_codex_config_path(None)?;
    let Some((requires_openai_auth, catalog_path)) = managed_catalog_settings(&config_path)? else {
        println!("Codex model catalog is not managed by codex-mixin");
        return Ok(());
    };
    let gateway_config = GatewayConfig::from_env()?;
    let state = AppState::new(gateway_config.clone())?;
    let models = state.fetch_models().await?;
    if models.is_empty() {
        anyhow::bail!("upstream /v1/models returned no models");
    }
    let codex_home = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?;
    let template = if requires_openai_auth {
        let models_cache = codex_home.join("models_cache.json");
        let template = load_template_catalog(Some(&models_cache))?;
        if template.is_none() {
            anyhow::bail!(
                "official Codex model cache is missing: {}. Open Codex once before refreshing Codex Mixin",
                models_cache.display()
            );
        }
        template
    } else {
        None
    };
    let metadata = load_model_metadata_resolver().await?;
    let catalog = if requires_openai_auth {
        codex_oauth_proxy_catalog_from_models_with_metadata_for_provider(
            &models,
            gateway_config.default_context_window,
            template.as_ref(),
            &metadata,
            gateway_config.provider_preset.as_str(),
        )
    } else {
        codex_catalog_from_models_with_metadata(
            &models,
            gateway_config.default_context_window,
            template.as_ref(),
            &metadata,
        )
    };
    let supported_models =
        WebSearchCapabilities::from_default_path(&gateway_config)?.supported_model_ids();
    if write_generated_managed_codex_catalog(&config_path, catalog, &supported_models)? {
        println!("Codex model catalog refreshed: {}", catalog_path.display());
    } else {
        println!(
            "Codex model catalog already current: {}",
            catalog_path.display()
        );
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
    if !is_managed_config(&raw_config) {
        return Ok(None);
    }
    let doc = raw_config.parse::<DocumentMut>()?;
    let provider = doc
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(CODEX_MIXIN_PROVIDER))
        .and_then(Item::as_table)
        .ok_or_else(|| anyhow::anyhow!("managed Codex config has no codex-mixin provider"))?;
    let requires_openai_auth = match provider.get("requires_openai_auth") {
        Some(item) => item
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("codex-mixin requires_openai_auth must be a boolean"))?,
        None => false,
    };
    let catalog_path = managed_catalog_path(&doc, &config_path)?;
    Ok(Some((requires_openai_auth, catalog_path)))
}

pub(in crate::cli) fn write_generated_managed_codex_catalog(
    config_path: &Path,
    mut catalog: serde_json::Value,
    supported_web_search_models: &HashSet<String>,
) -> anyhow::Result<bool> {
    let config_path = absolute_path(config_path.to_path_buf())?;
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
    let raw_config = fs::read_to_string(&config_path)?;
    if !is_managed_config(&raw_config) {
        return Ok(false);
    }
    let doc = raw_config.parse::<DocumentMut>()?;
    let catalog_path = managed_catalog_path(&doc, &config_path)?;
    apply_web_search_capabilities(&mut catalog, supported_web_search_models)?;
    write_atomic_if_changed(&catalog_path, &serde_json::to_vec_pretty(&catalog)?)
}

#[cfg(test)]
pub(in crate::cli) fn refresh_managed_codex_catalog(config_path: &Path) -> anyhow::Result<bool> {
    refresh_managed_codex_catalog_with_capabilities(config_path, None)
}

pub(in crate::cli) fn refresh_managed_codex_catalog_with_capabilities(
    config_path: &Path,
    supported_web_search_models: Option<&HashSet<String>>,
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
    if !is_managed_config(&raw_config) {
        return Ok(false);
    }
    let doc = raw_config.parse::<DocumentMut>()?;
    let provider = doc
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(CODEX_MIXIN_PROVIDER))
        .and_then(Item::as_table)
        .ok_or_else(|| anyhow::anyhow!("managed Codex config has no codex-mixin provider"))?;
    let requires_openai_auth = match provider.get("requires_openai_auth") {
        Some(item) => item
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("codex-mixin requires_openai_auth must be a boolean"))?,
        None => false,
    };
    if !requires_openai_auth && supported_web_search_models.is_none() {
        return Ok(false);
    }
    let catalog_path = managed_catalog_path(&doc, &config_path)?;
    let managed_catalog = serde_json::from_slice(&fs::read(&catalog_path)?)?;
    let mut refreshed = if requires_openai_auth {
        let official_catalog_path = config_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?
            .join("models_cache.json");
        let official_catalog = serde_json::from_slice(&fs::read(&official_catalog_path)?)?;
        refresh_managed_oauth_catalog(&official_catalog, &managed_catalog)?
    } else {
        managed_catalog
    };
    if let Some(supported_web_search_models) = supported_web_search_models {
        apply_web_search_capabilities(&mut refreshed, supported_web_search_models)?;
    }
    write_atomic_if_changed(&catalog_path, &serde_json::to_vec_pretty(&refreshed)?)
}

pub(in crate::cli) fn load_codex_install_template(
    paths: &CodexInstallPaths,
    codex_oauth_proxy: bool,
) -> anyhow::Result<Option<serde_json::Value>> {
    if !codex_oauth_proxy {
        return Ok(None);
    }
    let template = load_template_catalog(Some(&paths.models_cache))?;
    if template.is_none() {
        anyhow::bail!(
            "official Codex model cache is missing: {}. Open Codex once before installing Codex Mixin",
            paths.models_cache.display()
        );
    }
    Ok(template)
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
    provider_suffix: &str,
) -> anyhow::Result<String> {
    if let Some(model) = requested_model {
        if model_exists_in_oauth_proxy_catalog(&model, models, template_catalog) {
            return Ok(model);
        }
        if let Some(canonical) = strip_provider_suffix(&model, provider_suffix)
            && is_gpt_model(canonical)
            && models.iter().any(|candidate| candidate.id == canonical)
        {
            return Ok(model);
        }
        if is_gpt_model(&model) && models.iter().any(|candidate| candidate.id == model) {
            return Ok(format!("{model}-{provider_suffix}"));
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
    if let Some(model) = models.iter().find(|model| model.id == "Claude Sonnet 5") {
        return Ok(model.id.clone());
    }
    let first = &models[0].id;
    if is_gpt_model(first) {
        Ok(format!("{first}-{provider_suffix}"))
    } else {
        Ok(first.clone())
    }
}

pub(in crate::cli) fn model_exists_in_oauth_proxy_catalog(
    model: &str,
    models: &[codex_mixin::anthropic::ModelInfo],
    template_catalog: Option<&serde_json::Value>,
) -> bool {
    if template_catalog_has_model(template_catalog, model) {
        return true;
    }
    if let Some(canonical) = ProviderPreset::strip_model_provider_suffix(model)
        && is_gpt_model(canonical)
    {
        return models.iter().any(|candidate| candidate.id == canonical);
    }
    models
        .iter()
        .any(|candidate| candidate.id == model && !is_gpt_model(&candidate.id))
}

pub(in crate::cli) fn strip_provider_suffix<'a>(
    model: &'a str,
    provider_suffix: &str,
) -> Option<&'a str> {
    model.strip_suffix(&format!("-{provider_suffix}"))
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
