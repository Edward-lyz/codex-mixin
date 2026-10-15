//! Explicit OAuth login owns credentials without rewriting other applications' auth.
use anyhow::ensure;

pub async fn login(id: &str, show_url: impl Fn(&str)) -> anyhow::Result<()> {
    let preset = if id == "official" {
        "official"
    } else {
        let stored = crate::config::load_stored_config()?.unwrap_or_default();
        if let Some(provider) = stored.providers.iter().find(|provider| provider.id == id) {
            ensure!(
                provider.preset_id.as_deref() == Some("agy"),
                "OAuth login requires an AGY or official provider"
            );
        } else {
            ensure!(id == "agy", "provider {id} does not exist");
        }
        "agy"
    };
    crate::provider::auth::oauth::login(preset, show_url).await?;
    crate::config::mutate_stored_config(|config| {
        if preset == "agy" {
            if let Some(provider) = config
                .providers
                .iter_mut()
                .find(|provider| provider.id == id)
            {
                provider.auth.api_key.clear();
            } else {
                config.providers.push(crate::provider::agy_provider(id, ""));
            }
            // Native AGY providers share this OAuth session. Do not retain
            // another account's routable directory after an explicit login.
            for provider in config.providers.iter_mut().filter(|provider| {
                provider.preset_id.as_deref() == Some("agy")
                    && provider.auth.api_key.trim().is_empty()
            }) {
                provider.cached_models.clear();
                provider.models_refreshed_at_ms = None;
                provider.models_refresh_error =
                    Some("AGY OAuth account changed; refresh the model directory".to_owned());
            }
        }
        Ok(())
    })
    .map_err(|source| super::super::error::OperationError::AfterCommit {
        stage: "OAuth provider configuration",
        source,
    })?;
    Ok(())
}
