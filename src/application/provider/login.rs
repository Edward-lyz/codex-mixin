//! Explicit OAuth login owns credentials without rewriting other applications' auth.
use crate::provider::ProviderPreset;

pub async fn authorize_add(
    preset: ProviderPreset,
    timeout_seconds: u64,
    show_url: impl Fn(&str),
) -> anyhow::Result<()> {
    let profile = match preset {
        ProviderPreset::Official => "official",
        ProviderPreset::Agy => "agy",
        _ => anyhow::bail!("this provider preset does not use browser OAuth"),
    };
    crate::provider::auth::oauth::login(profile, timeout_seconds, show_url).await?;
    if preset == ProviderPreset::Official {
        return Ok(());
    }
    crate::config::mutate_stored_config(|config| {
        // Native AGY providers share this OAuth session. Do not retain
        // another account's routable directory after an explicit login.
        for provider in config.providers.iter_mut().filter(|provider| {
            provider.preset_id.as_deref() == Some("agy") && provider.auth.api_key.trim().is_empty()
        }) {
            provider.cached_models.clear();
            provider.models_refreshed_at_ms = None;
            provider.models_refresh_error =
                Some("AGY OAuth account changed; refresh the model directory".to_owned());
        }
        Ok(())
    })
    .map_err(|source| super::super::error::OperationError::AfterCommit {
        stage: "OAuth provider configuration",
        source,
    })?;
    Ok(())
}
