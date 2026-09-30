use codex_mixin::application::fusion;
use codex_mixin::fusion::FusionProfile;
use serde_json::json;

pub(super) fn get_fusion_profile(id: Option<&str>, json_output: bool) -> anyhow::Result<()> {
    let profile = fusion::get_profile(id)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "profile": profile }))?
        );
    } else if let Some(profile) = profile {
        println!("{}", serde_json::to_string_pretty(&profile)?);
    } else {
        println!("no fusion profile configured");
    }
    Ok(())
}

pub(super) fn set_fusion_profile(
    profile_json: &str,
    replace_id: Option<&str>,
) -> anyhow::Result<()> {
    let profile: FusionProfile = serde_json::from_str(profile_json)?;
    let id = fusion::set_profile(profile, replace_id)?;
    println!("fusion profile saved: {id}");
    Ok(())
}

pub(super) fn delete_fusion_profile(id: Option<&str>) -> anyhow::Result<()> {
    let id = fusion::delete_profile(id)?;
    println!("fusion profile deleted: {id}");
    Ok(())
}

pub(super) fn model_options(json_output: bool) -> anyhow::Result<()> {
    let config = codex_mixin::config::GatewayConfig::from_stored_config()?;
    let official = super::official_models::selected_official_models(&config)?;
    let models = fusion::model_options(&config.providers, &official)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"models": models}))?
        );
    } else {
        for model in models {
            println!("{}: {}", model.id, model.display_name);
        }
    }
    Ok(())
}
