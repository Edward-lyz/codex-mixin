use std::time::Duration;

use anyhow::Context;
use codex_mixin::config::GatewayConfig;
use codex_mixin::provider::{
    ProviderModelSource, apply_discovered_models, discover_provider_models, redact_provider_error,
};
use codex_mixin::provider_capabilities::ProviderCapabilities;
use serde_json::json;

use super::{
    TestProviderOptions, apply_baidu_auth_options, discovery::apply_discovered_quota,
    discovery::apply_inferred_custom_endpoint, discovery::detect_custom_provider_protocol,
    discovery::discover_custom_quota, discovery_settings_match, ensure_has_providers,
    find_provider_mut, mutate_and_invalidate, normalize_base_url, normalize_model_ids,
    required_config, trim_required,
};
use crate::cli::official_models::{
    OFFICIAL_PROVIDER_ID, available_official_ids, load_official_models,
};

pub(crate) async fn discover_models(id: &str) -> anyhow::Result<()> {
    discover_models_with_output(id, false).await
}

pub(crate) async fn discover_models_with_output(id: &str, quiet: bool) -> anyhow::Result<()> {
    let config = required_config()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?
        .clone();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    super::super::progress_step(&format!("Refreshing model list for provider {id}"));
    let quota_probe = async {
        if provider.preset_id.as_deref() == Some("custom") && provider.quota_url.is_none() {
            discover_custom_quota(&client, &provider).await
        } else {
            Ok(None)
        }
    };
    let (models, discovered_quota) =
        tokio::join!(discover_provider_models(&client, &provider), quota_probe);
    let discovered_quota = match discovered_quota {
        Ok(discovered) => discovered,
        Err(error) => {
            tracing::warn!(
                provider_id = provider.id,
                error = %redact_provider_error(&provider, &format!("{error:#}")),
                "custom quota discovery failed"
            );
            None
        }
    };
    let models = match models {
        Ok(models) => models,
        Err(error) => {
            let stored_error = redact_provider_error(&provider, &format!("{error:#}"));
            super::super::progress_step(&format!(
                "Model refresh failed for {id}: {}",
                stored_error
                    .lines()
                    .next()
                    .unwrap_or("model discovery failed")
            ));
            mutate_and_invalidate(|config| {
                let current = find_provider_mut(config, id)?;
                anyhow::ensure!(
                    discovery_settings_match(current, &provider),
                    "provider {id} discovery settings changed during refresh; retry"
                );
                current.models_refresh_error = Some(stored_error);
                if let Some(discovered_quota) = &discovered_quota {
                    apply_discovered_quota(current, discovered_quota);
                }
                Ok(())
            })?;
            return Err(error);
        }
    };
    super::super::progress_step(&format!(
        "Discovered {} models for provider {id}",
        models.len()
    ));
    let runtime_config = GatewayConfig::from_stored_config()?;
    let capabilities = ProviderCapabilities::from_default_path(&runtime_config)?;
    let mut annotated_provider = provider.clone();
    annotated_provider.cached_models = models;
    capabilities.annotate_provider(&mut annotated_provider);
    let models = annotated_provider.cached_models;
    let count = models.len();
    mutate_and_invalidate(|config| {
        let current = find_provider_mut(config, id)?;
        anyhow::ensure!(
            discovery_settings_match(current, &provider),
            "provider {id} discovery settings changed during refresh; retry"
        );
        if let Some(discovered_quota) = &discovered_quota {
            apply_discovered_quota(current, discovered_quota);
        }
        apply_discovered_models(current, models)
    })?;
    super::super::progress_step(&format!(
        "Model refresh complete for {id}: {count} available"
    ));
    if !quiet {
        println!("provider models refreshed: {id} ({count} available)");
        if let Some(discovered_quota) = discovered_quota {
            println!(
                "provider quota endpoint detected: {id} ({})",
                discovered_quota.url
            );
        }
    }
    Ok(())
}

pub(crate) async fn probe_selected_models(id: &str) -> anyhow::Result<()> {
    let config = required_config()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?
        .clone();
    let selected_models = provider
        .cached_models
        .iter()
        .filter(|model| {
            provider
                .selected_models
                .iter()
                .any(|selected| selected == &model.id)
        })
        .cloned()
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !selected_models.is_empty(),
        "provider {id} has no selected cached models; refresh the model list and select models first"
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    super::super::progress_step(&format!(
        "Probing {} selected models for provider {id}",
        selected_models.len()
    ));
    let provider_id = provider.id.clone();
    let summary = ProviderCapabilities::probe_provider_with_progress(
        client,
        &provider,
        &selected_models,
        Some(std::sync::Arc::new(move |done, total, supported, indeterminate| {
            super::super::progress_step(&format!(
                "Probing capabilities for {provider_id}: {done}/{total} complete ({supported} routed, {indeterminate} indeterminate)"
            ));
        })),
    )
    .await?;
    let runtime_config = GatewayConfig::from_stored_config()?;
    let mut capabilities = ProviderCapabilities::from_default_path(&runtime_config)?;
    capabilities.replace_provider_results(&provider, &runtime_config, &summary.results)?;
    mutate_and_invalidate(|config| {
        let current = find_provider_mut(config, id)?;
        anyhow::ensure!(
            discovery_settings_match(current, &provider),
            "provider {id} settings changed during capability probing; retry"
        );
        capabilities.annotate_provider(current);
        current.validate()
    })?;
    super::super::progress_step(&format!(
        "Capability probing complete for {id}: {} models checked",
        summary.attempted
    ));
    println!(
        "provider capabilities probed: {id} ({} models checked)",
        summary.attempted
    );
    Ok(())
}

pub(crate) async fn test_provider(options: TestProviderOptions) -> anyhow::Result<()> {
    let id = options.id.as_str();
    let config = required_config()?;
    let stored_provider = config
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
    let mut provider = stored_provider.clone();
    if let Some(key) = options.key {
        provider.auth.api_key = trim_required("key", key)?;
    }
    if let Some(base_url) = options.base_url {
        provider.base_url = normalize_base_url(base_url)?;
    }
    apply_baidu_auth_options(
        &mut provider,
        options.baidu_auth_bridge.as_deref(),
        options.ducx_executable,
    )?;
    if provider.preset_id.as_deref() == Some("custom")
        && !matches!(&provider.model_source, ProviderModelSource::Static)
    {
        let endpoint = detect_custom_provider_protocol(&provider)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("custom provider endpoint detection returned no result")
            })?;
        apply_inferred_custom_endpoint(&mut provider, endpoint);
    }
    provider.validate()?;
    let (mode, model_count) = match &provider.model_source {
        ProviderModelSource::Static => ("configuration", provider.cached_models.len()),
        _ => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;
            let models = discover_provider_models(&client, &provider)
                .await
                .with_context(|| {
                    format!(
                        "provider test failed for {id}; check the API key, base URL, and network"
                    )
                })?;
            ("models_endpoint", models.len())
        }
    };
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider_id": provider.id,
                "ok": true,
                "mode": mode,
                "model_count": model_count,
                "paid_inference_performed": false,
            }))?
        );
    } else if mode == "configuration" {
        println!(
            "provider test ok: {id} (static model source; configuration only, no paid inference)"
        );
    } else {
        println!("provider test ok: {id} ({model_count} models)");
    }
    Ok(())
}

pub(crate) fn select_models(id: &str, models: Vec<String>) -> anyhow::Result<()> {
    let models = normalize_model_ids(models)?;
    let selected_count = models.len();
    if id == OFFICIAL_PROVIDER_ID {
        let available_models = load_official_models()?;
        let available_ids = available_official_ids(&available_models);
        for model in &models {
            anyhow::ensure!(
                available_ids.contains(model.as_str()),
                "official provider has no known model {model}; open Codex once to refresh its model cache"
            );
        }
        mutate_and_invalidate(|config| {
            config.official_selected_models = Some(models);
            Ok(())
        })?;
        println!("provider models selected: {id} ({selected_count})");
        return Ok(());
    }
    mutate_and_invalidate(|config| {
        ensure_has_providers(config)?;
        apply_model_selection(find_provider_mut(config, id)?, models)
    })?;
    println!("provider models selected: {id} ({selected_count})");
    Ok(())
}

pub(super) fn apply_model_selection(
    provider: &mut codex_mixin::provider::ProviderDefinition,
    models: Vec<String>,
) -> anyhow::Result<()> {
    let allowed = provider
        .cached_models
        .iter()
        .map(|model| model.id.as_str())
        .chain(provider.selected_models.iter().map(String::as_str))
        .collect::<std::collections::HashSet<_>>();
    for model in &models {
        if !allowed.contains(model.as_str()) {
            anyhow::bail!(
                "provider {} has no known model {model}; run discover first",
                provider.id
            );
        }
    }
    provider.selected_models = models;
    provider.new_models.clear();
    provider.validate()
}
