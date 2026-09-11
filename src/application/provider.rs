use std::collections::{HashMap, HashSet};

use crate::config::{StoredGatewayConfig, mutate_stored_config};
use crate::provider::capabilities::ProviderCapabilities;
use crate::provider::{
    ModelDiscoveryChanges, ProviderDefinition, ProviderModel, ProviderPreset, ProviderProtocol,
    ProviderQuotaParser, apply_discovered_models, catalog_model_slug,
};
use crate::web_search::WebSearchCapabilities;

use super::error::OperationError;

pub fn commit_provider_change<T>(
    mutation: impl FnOnce(&mut StoredGatewayConfig) -> anyhow::Result<T>,
) -> Result<T, OperationError> {
    mutate_stored_config(mutation).map_err(|source| OperationError::BeforeCommit { source })
}

pub fn after_provider_commit<T>(
    stage: &'static str,
    action: impl FnOnce() -> anyhow::Result<T>,
) -> Result<T, OperationError> {
    action().map_err(|source| OperationError::AfterCommit { stage, source })
}

pub async fn after_provider_commit_async<T>(
    stage: &'static str,
    action: impl std::future::Future<Output = anyhow::Result<T>>,
) -> Result<T, OperationError> {
    action
        .await
        .map_err(|source| OperationError::AfterCommit { stage, source })
}

#[derive(Debug, Eq, PartialEq)]
pub struct RemovedProvider {
    pub renames: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct DiscoveredQuota {
    pub url: String,
    pub parser: ProviderQuotaParser,
    pub currency: Option<String>,
}

pub fn provider_for_refresh(id: &str) -> anyhow::Result<ProviderDefinition> {
    let config = crate::config::load_stored_config()?
        .ok_or_else(|| anyhow::anyhow!("provider configuration is missing"))?;
    config
        .providers
        .into_iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))
}

pub fn record_refresh_failure(
    id: &str,
    snapshot: &ProviderDefinition,
    stored_error: String,
    quota: Option<&DiscoveredQuota>,
) -> anyhow::Result<()> {
    mutate_stored_config(|config| {
        let current = provider_mut(config, id)?;
        ensure_refresh_snapshot(current, snapshot, id)?;
        current.models_refresh_error = Some(stored_error);
        if let Some(quota) = quota {
            apply_quota(current, quota);
        }
        Ok(())
    })?;
    WebSearchCapabilities::clear_default_cache()?;
    Ok(())
}

pub fn commit_discovered_models(
    id: &str,
    snapshot: &ProviderDefinition,
    models: Vec<ProviderModel>,
    quota: Option<&DiscoveredQuota>,
) -> anyhow::Result<ModelDiscoveryChanges> {
    let changes = mutate_stored_config(|config| {
        let current = provider_mut(config, id)?;
        ensure_refresh_snapshot(current, snapshot, id)?;
        if let Some(quota) = quota {
            apply_quota(current, quota);
        }
        apply_discovered_models(current, models)
    })?;
    WebSearchCapabilities::clear_default_cache()?;
    Ok(changes)
}

pub fn commit_detected_endpoint(
    id: &str,
    snapshot: &ProviderDefinition,
    base_url: String,
    protocol: ProviderProtocol,
    api_path: String,
    models_path: String,
) -> Result<(), OperationError> {
    mutate_stored_config(|config| {
        apply_detected_endpoint(
            config,
            id,
            snapshot,
            base_url,
            protocol,
            api_path,
            models_path,
        )
    })
    .map_err(|source| OperationError::AfterCommit {
        stage: "detected protocol persistence",
        source,
    })?;
    invalidate_provider_caches()
}

#[allow(clippy::too_many_arguments)]
fn apply_detected_endpoint(
    config: &mut StoredGatewayConfig,
    id: &str,
    snapshot: &ProviderDefinition,
    base_url: String,
    protocol: ProviderProtocol,
    api_path: String,
    models_path: String,
) -> anyhow::Result<()> {
    let current = provider_mut(config, id)?;
    anyhow::ensure!(
        current == snapshot,
        "provider {id} changed during protocol detection; retry"
    );
    current.base_url = base_url;
    current.protocol = protocol;
    current.api_path = api_path;
    current.model_source =
        crate::provider::ProviderModelSource::OpenAiCompatible { path: models_path };
    current.validate()
}

fn ensure_refresh_snapshot(
    current: &ProviderDefinition,
    snapshot: &ProviderDefinition,
    id: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        current.base_url == snapshot.base_url
            && current.model_source == snapshot.model_source
            && current.auth == snapshot.auth,
        "provider {id} discovery settings changed during refresh; retry"
    );
    Ok(())
}

fn apply_quota(provider: &mut ProviderDefinition, quota: &DiscoveredQuota) {
    provider.quota_url = Some(quota.url.clone());
    provider.quota_parser = quota.parser;
    provider.quota_currency.clone_from(&quota.currency);
}

pub fn set_provider_enabled(id: &str, enabled: bool) -> Result<(), OperationError> {
    commit_provider_change(|config| {
        require_providers(config)?;
        provider_mut(config, id)?.enabled = enabled;
        Ok(())
    })?;
    invalidate_provider_caches()
}

pub fn add_provider(
    provider: ProviderDefinition,
    gateway_api_key: Option<String>,
) -> Result<(), OperationError> {
    commit_provider_change(|config| add_provider_config(config, provider, gateway_api_key))?;
    invalidate_provider_caches()
}

pub fn add_provider_config(
    config: &mut StoredGatewayConfig,
    provider: ProviderDefinition,
    gateway_api_key: Option<String>,
) -> anyhow::Result<()> {
    let id = &provider.id;
    anyhow::ensure!(
        !config.providers.iter().any(|provider| &provider.id == id),
        "provider already exists: {id}"
    );
    if gateway_api_key.is_some() {
        config.gateway_api_key = gateway_api_key;
    }
    if provider.auxiliary_model_upstream {
        for existing in &mut config.providers {
            existing.auxiliary_model_upstream = false;
        }
    }
    config.providers.push(provider);
    Ok(())
}

pub fn update_provider(
    id: &str,
    snapshot: &ProviderDefinition,
    provider: ProviderDefinition,
    auxiliary_model_upstream: Option<bool>,
) -> Result<(), OperationError> {
    commit_provider_change(|config| {
        update_provider_config(config, id, snapshot, provider, auxiliary_model_upstream)
    })?;
    invalidate_provider_caches()
}

pub fn update_provider_config(
    config: &mut StoredGatewayConfig,
    id: &str,
    snapshot: &ProviderDefinition,
    provider: ProviderDefinition,
    auxiliary_model_upstream: Option<bool>,
) -> anyhow::Result<()> {
    require_providers(config)?;
    let current = provider_mut(config, id)?;
    anyhow::ensure!(
        current == snapshot,
        "provider {id} changed during update; retry"
    );
    *current = provider;
    if let Some(enabled) = auxiliary_model_upstream {
        set_auxiliary_upstream(config, id, enabled)?;
    }
    Ok(())
}

pub fn reorder_providers(ids: &[String]) -> Result<(), OperationError> {
    commit_provider_change(|config| reorder_provider_config(config, ids))?;
    invalidate_web_search_cache()
}

pub fn remove_provider(id: &str) -> Result<RemovedProvider, OperationError> {
    let renames = commit_provider_change(|config| {
        require_providers(config)?;
        remove_provider_config(config, id)
    })?;
    invalidate_provider_caches()?;
    Ok(RemovedProvider { renames })
}

pub fn set_auxiliary_upstream(
    config: &mut StoredGatewayConfig,
    id: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    require_providers(config)?;
    let selected = config
        .providers
        .iter()
        .position(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
    if enabled {
        for provider in &mut config.providers {
            provider.auxiliary_model_upstream = false;
        }
    }
    config.providers[selected].auxiliary_model_upstream = enabled;
    Ok(())
}

fn invalidate_web_search_cache() -> Result<(), OperationError> {
    after_provider_commit("web search capability cache invalidation", || {
        WebSearchCapabilities::clear_default_cache().map(|_| ())
    })
}

fn invalidate_provider_caches() -> Result<(), OperationError> {
    invalidate_web_search_cache()?;
    after_provider_commit("provider capability cache invalidation", || {
        ProviderCapabilities::clear_default_cache().map(|_| ())
    })
}

fn require_providers(config: &StoredGatewayConfig) -> anyhow::Result<()> {
    anyhow::ensure!(
        !config.providers.is_empty(),
        "provider configuration is missing"
    );
    Ok(())
}

fn provider_mut<'a>(
    config: &'a mut StoredGatewayConfig,
    id: &str,
) -> anyhow::Result<&'a mut crate::provider::ProviderDefinition> {
    config
        .providers
        .iter_mut()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))
}

pub fn reorder_provider_config(
    config: &mut StoredGatewayConfig,
    ids: &[String],
) -> anyhow::Result<()> {
    require_providers(config)?;
    anyhow::ensure!(
        ids.len() == config.providers.len(),
        "provider reorder requires all {} provider IDs in the desired order",
        config.providers.len()
    );
    let mut seen = HashSet::new();
    for id in ids {
        anyhow::ensure!(
            seen.insert(id),
            "provider reorder contains duplicate provider ID: {id}"
        );
        anyhow::ensure!(
            config.providers.iter().any(|provider| provider.id == *id),
            "unknown provider: {id}"
        );
    }
    let mut reordered = Vec::with_capacity(config.providers.len());
    let mut remaining = std::mem::take(&mut config.providers);
    for id in ids {
        let index = remaining
            .iter()
            .position(|provider| provider.id == *id)
            .ok_or_else(|| anyhow::anyhow!("unknown or duplicate provider ID: {id}"))?;
        reordered.push(remaining.remove(index));
    }
    config.providers = reordered;
    Ok(())
}

pub fn remove_provider_config(
    config: &mut StoredGatewayConfig,
    id: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    let index = config
        .providers
        .iter()
        .position(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
    let removed = config.providers.remove(index);
    let Some(preset_id) = removed.preset_id else {
        return Ok(Vec::new());
    };
    compact_generated_provider_ids(config, ProviderPreset::parse(&preset_id)?)
}

fn compact_generated_provider_ids(
    config: &mut StoredGatewayConfig,
    preset: ProviderPreset,
) -> anyhow::Result<Vec<(String, String)>> {
    let base_id = preset.default_id();
    let mut generated = config
        .providers
        .iter()
        .enumerate()
        .filter_map(|(index, provider)| {
            (provider.preset_id.as_deref() == Some(preset.as_str()))
                .then(|| generated_provider_ordinal(&provider.id, base_id))
                .flatten()
                .map(|ordinal| (ordinal, index))
        })
        .collect::<Vec<_>>();
    generated.sort_unstable_by_key(|(ordinal, _)| *ordinal);
    let renames = generated
        .into_iter()
        .enumerate()
        .filter_map(|(position, (_, provider_index))| {
            let new_id = if position == 0 {
                base_id.to_owned()
            } else {
                format!("{base_id}-{}", position + 1)
            };
            let old_id = config.providers[provider_index].id.clone();
            (old_id != new_id).then_some((provider_index, old_id, new_id))
        })
        .collect::<Vec<_>>();
    let mut model_renames = HashMap::new();
    for (provider_index, old_id, new_id) in &renames {
        let provider = &config.providers[*provider_index];
        for model_id in provider
            .selected_models
            .iter()
            .chain(provider.cached_models.iter().map(|model| &model.id))
        {
            model_renames.insert(
                catalog_model_slug(model_id, old_id),
                catalog_model_slug(model_id, new_id),
            );
        }
    }
    for (provider_index, _, new_id) in &renames {
        config.providers[*provider_index].id.clone_from(new_id);
    }
    for profile in &mut config.fusion_profiles {
        for reference in profile
            .panel_models
            .iter_mut()
            .chain([&mut profile.judge_model, &mut profile.final_model])
        {
            if let Some(new_reference) = model_renames.get(reference) {
                reference.clone_from(new_reference);
            }
        }
        for route in &mut profile.time_routes {
            if let Some(new_reference) = model_renames.get(&route.model) {
                route.model.clone_from(new_reference);
            }
        }
    }
    Ok(renames
        .into_iter()
        .map(|(_, old_id, new_id)| (old_id, new_id))
        .collect())
}

fn generated_provider_ordinal(id: &str, base_id: &str) -> Option<usize> {
    if id == base_id {
        return Some(1);
    }
    id.strip_prefix(base_id)?
        .strip_prefix('-')?
        .parse::<usize>()
        .ok()
        .filter(|ordinal| *ordinal >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str) -> ProviderDefinition {
        ProviderPreset::DeepSeek.create(id.to_owned(), "secret".to_owned())
    }

    #[test]
    fn add_provider_keeps_auxiliary_selection_exclusive() {
        let mut config = StoredGatewayConfig::default();
        let mut existing = provider("existing");
        existing.auxiliary_model_upstream = true;
        config.providers.push(existing);
        let mut added = provider("added");
        added.auxiliary_model_upstream = true;

        add_provider_config(&mut config, added, Some("gateway".to_owned())).unwrap();

        assert!(!config.providers[0].auxiliary_model_upstream);
        assert!(config.providers[1].auxiliary_model_upstream);
        assert_eq!(config.gateway_api_key.as_deref(), Some("gateway"));
    }

    #[test]
    fn update_provider_rejects_a_stale_snapshot_without_overwriting() {
        let snapshot = provider("provider");
        let mut config = StoredGatewayConfig {
            providers: vec![snapshot.clone()],
            ..StoredGatewayConfig::default()
        };
        config.providers[0].display_name = "concurrent edit".to_owned();
        let mut replacement = snapshot.clone();
        replacement.display_name = "requested edit".to_owned();

        let error = update_provider_config(&mut config, "provider", &snapshot, replacement, None)
            .unwrap_err();

        assert!(error.to_string().contains("changed during update"));
        assert_eq!(config.providers[0].display_name, "concurrent edit");
    }

    #[test]
    fn protocol_detection_rejects_a_stale_snapshot() {
        let snapshot = provider("provider");
        let mut config = StoredGatewayConfig {
            providers: vec![snapshot.clone()],
            ..StoredGatewayConfig::default()
        };
        config.providers[0].base_url = "https://concurrent.example".to_owned();

        let error = apply_detected_endpoint(
            &mut config,
            "provider",
            &snapshot,
            "https://detected.example".to_owned(),
            ProviderProtocol::OpenAiChat,
            "/v1/chat/completions".to_owned(),
            "/v1/models".to_owned(),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed during protocol detection")
        );
        assert_eq!(config.providers[0].base_url, "https://concurrent.example");
    }
}
