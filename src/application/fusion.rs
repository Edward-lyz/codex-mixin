use crate::config::{StoredGatewayConfig, load_stored_config, mutate_stored_config};
use crate::fusion::FusionProfile;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fusion::PanelToolsConfig;

    fn profile(id: &str) -> FusionProfile {
        FusionProfile {
            id: id.to_owned(),
            mode: Default::default(),
            time_routes: Vec::new(),
            panel_models: vec!["model-provider".to_owned()],
            judge_model: "model-provider".to_owned(),
            final_model: "model-provider".to_owned(),
            min_successful: 1,
            max_completion_tokens: 2048,
            timeout_ms: 300_000,
            show_intermediate_results: true,
            panel_tools: PanelToolsConfig::default(),
        }
    }

    #[test]
    fn options_follow_routes_and_preserve_official_identity() {
        use crate::provider::{ProviderModel, custom_provider};
        let mut enabled = custom_provider("enabled", "secret");
        enabled.base_url = "https://example.com".to_owned();
        enabled.cached_models = vec![ProviderModel {
            id: "available".to_owned(),
            ..Default::default()
        }];
        enabled.selected_models = vec!["available".to_owned(), "missing".to_owned()];
        let mut disabled = enabled.clone();
        disabled.id = "disabled".to_owned();
        disabled.enabled = false;
        let official = ProviderModel {
            id: "available".to_owned(),
            ..Default::default()
        };
        let options = model_options(&[enabled, disabled], &[official]).unwrap();
        let ids = options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["available-enabled", "official:available"]);
        assert_eq!(options[1].provider_id, "official");
    }

    #[test]
    fn replaces_the_loaded_profile_when_its_id_changes() {
        let mut config = StoredGatewayConfig {
            fusion_profiles: vec![profile("old"), profile("other")],
            ..StoredGatewayConfig::default()
        };

        upsert_fusion_profile(&mut config, profile("renamed"), Some("old"));

        assert_eq!(
            config
                .fusion_profiles
                .iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>(),
            ["renamed", "other"]
        );
    }

    #[test]
    fn removes_the_requested_profile_and_keeps_the_rest() {
        let mut config = StoredGatewayConfig {
            fusion_profiles: vec![profile("old"), profile("other")],
            ..StoredGatewayConfig::default()
        };

        assert_eq!(
            remove_fusion_profile(&mut config, Some("old")).unwrap(),
            "old"
        );
        assert_eq!(
            config
                .fusion_profiles
                .iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>(),
            ["other"]
        );
    }

    #[test]
    fn delete_without_id_removes_the_first_profile() {
        let mut config = StoredGatewayConfig {
            fusion_profiles: vec![profile("first"), profile("second")],
            ..StoredGatewayConfig::default()
        };

        assert_eq!(remove_fusion_profile(&mut config, None).unwrap(), "first");
        assert_eq!(config.fusion_profiles[0].id, "second");
    }

    #[test]
    fn delete_missing_profile_fails() {
        let mut config = StoredGatewayConfig {
            fusion_profiles: vec![profile("default")],
            ..StoredGatewayConfig::default()
        };

        let error = remove_fusion_profile(&mut config, Some("missing")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("fusion profile not found: missing")
        );
        assert_eq!(config.fusion_profiles.len(), 1);
    }
}
