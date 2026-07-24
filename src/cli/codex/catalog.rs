use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use toml_edit::{DocumentMut, Item};

use codex_mixin::CODEX_MIXIN_PROVIDER;
use codex_mixin::catalog::{
    apply_web_search_capabilities, codex_catalog_from_models_with_metadata,
    codex_oauth_proxy_catalog_from_aggregated_models_with_metadata, load_template_catalog,
    refresh_managed_oauth_catalog,
};
use codex_mixin::config::GatewayConfig;
use codex_mixin::server::AppState;
use codex_mixin::web_search::WebSearchCapabilities;

use super::install::resolve_codex_cli;
use super::managed_config::*;
use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::metadata::load_model_metadata_resolver;

pub(in crate::cli) async fn refresh_default_managed_codex_catalog() -> anyhow::Result<()> {
    let config_path = resolve_codex_config_path(None)?;
    let Some((requires_openai_auth, catalog_path)) = managed_catalog_settings(&config_path)? else {
        println!("Codex model catalog is not managed by codex-mixin");
        return Ok(());
    };
    let gateway_config = GatewayConfig::from_stored_config()?;
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
        let template =
            load_preferred_official_catalog(&state, &models_cache, Some(&catalog_path)).await?;
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
        codex_oauth_proxy_catalog_from_aggregated_models_with_metadata(
            &models,
            gateway_config.default_context_window,
            template.as_ref(),
            &metadata,
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
    let Some((requires_openai_auth, _)) = managed_catalog_settings(config_path)? else {
        return Ok(false);
    };
    if !requires_openai_auth {
        return Ok(false);
    }
    let official_catalog_path = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?
        .join("models_cache.json");
    let official_catalog = serde_json::from_slice(&fs::read(official_catalog_path)?)?;
    refresh_managed_codex_catalog_from_official(config_path, &official_catalog, None)
}

pub(in crate::cli) fn refresh_managed_codex_catalog_with_capabilities(
    config_path: &Path,
    supported_web_search_models: Option<&HashSet<String>>,
) -> anyhow::Result<bool> {
    refresh_managed_codex_catalog_with_source(config_path, None, supported_web_search_models)
}

pub(in crate::cli) fn refresh_managed_codex_catalog_from_official(
    config_path: &Path,
    official_catalog: &serde_json::Value,
    supported_web_search_models: Option<&HashSet<String>>,
) -> anyhow::Result<bool> {
    refresh_managed_codex_catalog_with_source(
        config_path,
        Some(official_catalog),
        supported_web_search_models,
    )
}

fn refresh_managed_codex_catalog_with_source(
    config_path: &Path,
    official_catalog: Option<&serde_json::Value>,
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
    let mut refreshed = if requires_openai_auth && let Some(official_catalog) = official_catalog {
        refresh_managed_oauth_catalog(official_catalog, &managed_catalog)?
    } else {
        managed_catalog
    };
    if let Some(supported_web_search_models) = supported_web_search_models {
        apply_web_search_capabilities(&mut refreshed, supported_web_search_models)?;
    }
    write_atomic_if_changed(&catalog_path, &serde_json::to_vec_pretty(&refreshed)?)
}

pub(in crate::cli) async fn load_codex_install_template_online(
    paths: &CodexInstallPaths,
    codex_oauth_proxy: bool,
    state: &AppState,
) -> anyhow::Result<Option<serde_json::Value>> {
    if !codex_oauth_proxy {
        return Ok(None);
    }
    let template =
        load_preferred_official_catalog(state, &paths.models_cache, Some(&paths.catalog)).await?;
    if template.is_none() {
        anyhow::bail!(
            "official Codex model cache is missing: {}. Open Codex once before installing Codex Mixin",
            paths.models_cache.display()
        );
    }
    Ok(template)
}

#[cfg(test)]
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

pub(in crate::cli) async fn refresh_managed_official_codex_catalog(
    config_path: &Path,
    state: &AppState,
    supported_web_search_models: Option<&HashSet<String>>,
) -> anyhow::Result<bool> {
    let Some((requires_openai_auth, _)) = managed_catalog_settings(config_path)? else {
        return Ok(false);
    };
    if !requires_openai_auth {
        return Ok(false);
    }
    let models_cache = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?
        .join("models_cache.json");
    let client_version = resolve_codex_client_version(&models_cache)
        .ok_or_else(|| anyhow::anyhow!("Codex client version could not be determined"))?;
    let official_catalog = state.fetch_official_models_catalog(&client_version).await?;
    refresh_managed_codex_catalog_from_official(
        config_path,
        &official_catalog,
        supported_web_search_models,
    )
}

async fn load_preferred_official_catalog(
    state: &AppState,
    models_cache: &Path,
    managed_catalog: Option<&Path>,
) -> anyhow::Result<Option<serde_json::Value>> {
    if let Some(client_version) = resolve_codex_client_version(models_cache) {
        match state.fetch_official_models_catalog(&client_version).await {
            Ok(catalog) => return Ok(Some(catalog)),
            Err(error) => {
                tracing::warn!(error = %error, "failed to fetch official Codex model catalog");
            }
        }
    } else {
        tracing::warn!("Codex client version could not be determined; using local model catalog");
    }
    if let Some(path) = managed_catalog
        && let Some(catalog) = load_current_official_catalog(path)?
    {
        return Ok(Some(catalog));
    }
    load_template_catalog(Some(models_cache))
}

fn load_current_official_catalog(path: &Path) -> anyhow::Result<Option<serde_json::Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut catalog: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    let Some(models) = catalog
        .get_mut("models")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(None);
    };
    models.retain(|model| !is_managed_catalog_model(model));
    Ok((!models.is_empty()).then_some(catalog))
}

fn is_managed_catalog_model(model: &serde_json::Value) -> bool {
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

fn resolve_codex_client_version(models_cache: &Path) -> Option<String> {
    if let Ok(codex_cli) = resolve_codex_cli()
        && let Ok(output) = ProcessCommand::new(codex_cli).arg("--version").output()
        && output.status.success()
        && let Some(version) = parse_codex_client_version(&String::from_utf8_lossy(&output.stdout))
    {
        return Some(version);
    }
    load_template_catalog(Some(models_cache))
        .ok()
        .flatten()
        .and_then(|catalog| {
            catalog
                .get("client_version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
}

pub(in crate::cli) fn parse_codex_client_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|part| {
            part.chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
        })
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
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
