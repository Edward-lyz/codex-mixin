use codex_mixin::config::{StoredGatewayConfig, load_stored_config, mutate_stored_config};
use codex_mixin::fusion::FusionProfile;
use codex_mixin::web_search::WebSearchCapabilities;
use serde_json::json;

pub(super) fn get_fusion_profile(id: Option<&str>, json_output: bool) -> anyhow::Result<()> {
    let stored = load_stored_config()?.unwrap_or_default();
    let profile = id.map_or_else(
        || stored.fusion_profiles.first(),
        |id| {
            stored
                .fusion_profiles
                .iter()
                .find(|profile| profile.id == id)
        },
    );
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "profile": profile }))?
        );
    } else if let Some(profile) = profile {
        println!("{}", serde_json::to_string_pretty(profile)?);
    } else {
        println!("no fusion profile configured");
    }
    Ok(())
}

pub(super) fn set_fusion_profile(
    profile_json: &str,
    replace_id: Option<&str>,
) -> anyhow::Result<()> {
    let profile: FusionProfile =
        serde_json::from_str(profile_json).map_err(|error| anyhow::anyhow!(error))?;
    profile.validate()?;
    let profile_id = profile.id.clone();
    mutate_stored_config(|config| {
        upsert_fusion_profile(config, profile, replace_id);
        Ok(())
    })?;
    WebSearchCapabilities::clear_default_cache()?;
    println!("fusion profile saved: {profile_id}");
    Ok(())
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
    use codex_mixin::fusion::PanelToolsConfig;

    fn profile(id: &str) -> FusionProfile {
        FusionProfile {
            id: id.to_owned(),
            panel_models: vec!["model-provider".to_owned()],
            judge_model: "model-provider".to_owned(),
            final_model: "model-provider".to_owned(),
            min_successful: 1,
            max_completion_tokens: 2048,
            timeout_ms: 300_000,
            fuse_every_user_turn: true,
            show_intermediate_results: true,
            panel_tools: PanelToolsConfig::default(),
        }
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
}
