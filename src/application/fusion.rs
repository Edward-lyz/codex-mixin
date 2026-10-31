use crate::config::FusionProfile;
use crate::config::{StoredGatewayConfig, load_stored_config, mutate_stored_config};
use crate::web_search::WebSearchCapabilities;

use super::error::OperationError;

pub fn get_profile(id: Option<&str>) -> anyhow::Result<Option<FusionProfile>> {
    let stored = load_stored_config()?.unwrap_or_default();
    Ok(match id {
        Some(id) => stored
            .fusion_profiles
            .into_iter()
            .find(|profile| profile.id == id),
        None => stored.fusion_profiles.into_iter().next(),
    })
}

pub fn set_profile(
    profile: FusionProfile,
    replace_id: Option<&str>,
) -> Result<String, OperationError> {
    profile
        .validate()
        .map_err(|source| OperationError::BeforeCommit { source })?;
    let id = profile.id.clone();
    mutate_stored_config(|config| {
        upsert_fusion_profile(config, profile, replace_id);
        Ok(())
    })
    .map_err(|source| OperationError::BeforeCommit { source })?;
    WebSearchCapabilities::clear_default_cache().map_err(|source| OperationError::AfterCommit {
        stage: "fusion cache invalidation",
        source,
    })?;
    Ok(id)
}

pub fn delete_profile(id: Option<&str>) -> Result<String, OperationError> {
    let removed = mutate_stored_config(|config| remove_fusion_profile(config, id))
        .map_err(|source| OperationError::BeforeCommit { source })?;
    WebSearchCapabilities::clear_default_cache().map_err(|source| OperationError::AfterCommit {
        stage: "fusion cache invalidation",
        source,
    })?;
    Ok(removed)
}

#[derive(Debug, serde::Serialize)]
pub struct ModelOption {
    pub id: String,
    pub display_name: String,
    pub provider_id: String,
}

/// Use the same resolved routes as the gateway; unavailable and disabled
/// provider models must not become selectable through a desktop shell.
pub fn model_options(
    providers: &[crate::provider::ProviderDefinition],
    official: &[crate::provider::ProviderModel],
) -> anyhow::Result<Vec<ModelOption>> {
    let registry = crate::provider::ProviderRegistry::new(providers.to_vec())?;
    let mut options = registry
        .routable_models()
        .map(|route| ModelOption {
            id: route.catalog_slug.to_owned(),
            display_name: route
                .model
                .and_then(|model| model.display_name.clone())
                .unwrap_or_else(|| route.upstream_model_id.to_owned()),
            provider_id: route.provider.id().to_owned(),
        })
        .collect::<Vec<_>>();
    options.extend(official.iter().map(|model| {
        ModelOption {
            id: format!("official:{}", model.id),
            display_name: model
                .display_name
                .clone()
                .unwrap_or_else(|| model.id.clone()),
            provider_id: "official".to_owned(),
        }
    }));
    options.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.id.cmp(&right.id))
    });
    Ok(options)
}

fn remove_fusion_profile(
    config: &mut StoredGatewayConfig,
    id: Option<&str>,
) -> anyhow::Result<String> {
    if config.fusion_profiles.is_empty() {
        anyhow::bail!("no fusion profile configured");
    }
    let index = match id {
        Some(id) => config
            .fusion_profiles
            .iter()
            .position(|profile| profile.id == id)
            .ok_or_else(|| anyhow::anyhow!("fusion profile not found: {id}"))?,
        None => 0,
    };
    Ok(config.fusion_profiles.remove(index).id)
}

fn upsert_fusion_profile(
    config: &mut StoredGatewayConfig,
    profile: FusionProfile,
    replace_id: Option<&str>,
) {
    let replace_index = replace_id
        .and_then(|id| {
            config
                .fusion_profiles
                .iter()
                .position(|current| current.id == id)
        })
        .or_else(|| {
            config
                .fusion_profiles
                .iter()
                .position(|current| current.id == profile.id)
        });
    if let Some(index) = replace_index {
        config.fusion_profiles[index] = profile;
    } else {
        config.fusion_profiles.push(profile);
    }
}
