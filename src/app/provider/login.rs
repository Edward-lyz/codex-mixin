//! Explicit OAuth login owns credentials without rewriting other applications' auth.
use crate::provider::ProviderPreset;
use anyhow::Context;

pub async fn authorize_add(
    preset: ProviderPreset,
    provider: Option<&crate::provider::ProviderDefinition>,
    timeout_seconds: u64,
    show_url: impl Fn(&str),
) -> anyhow::Result<()> {
    let profile = match preset {
        ProviderPreset::Official => "official",
        ProviderPreset::Agy => "agy",
        ProviderPreset::AgyBusiness => "agy-business",
        _ => anyhow::bail!("this provider preset does not use browser OAuth"),
    };
    let path = match provider {
        Some(provider) => provider
            .owned_oauth_path()
            .context("provider has no owned OAuth path")?,
        None => crate::platform::oauth::oauth_path(profile),
    };
    let previous = if preset == ProviderPreset::Official && provider.is_some() {
        match tokio::fs::read(&path).await {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("read existing OAuth credentials"),
        }
    } else {
        None
    };
    let previous_account = if previous.is_some() {
        // An unreadable credential is repaired by signing in again.
        match crate::provider::official_account_id(&path) {
            Ok(account) => account,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %crate::error::format_error_chain(error.as_ref()),
                    "existing OpenAI credentials are unreadable; replacing them"
                );
                None
            }
        }
    } else {
        None
    };
    crate::provider::auth::oauth::login(profile, &path, timeout_seconds, show_url).await?;
    if preset == ProviderPreset::Official {
        if let Some(provider) = provider {
            let account_id = crate::provider::official_account_id(&path)?
                .context("authorized OpenAI credentials do not contain an account ID")?;
            let swapped = previous_account
                .as_deref()
                .is_some_and(|previous| previous != account_id);
            let main_path = crate::platform::oauth::official_auth_path();
            let stored = crate::config::load_stored_config()?.unwrap_or_default();
            let mut duplicate = crate::provider::official_account_id(&main_path)?.as_deref()
                == Some(account_id.as_str());
            for other in stored.providers.iter().filter(|other| {
                other.id != provider.id && other.preset_id.as_deref() == Some("openai")
            }) {
                if let Some(other_path) = other.owned_oauth_path() {
                    duplicate |= crate::provider::official_account_id(&other_path)?.as_deref()
                        == Some(account_id.as_str());
                }
            }
            if duplicate || swapped {
                if let Some(previous) = previous {
                    let restore = path.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::platform::oauth::save_oauth(&restore, &previous)
                    })
                    .await?
                    .context("restore previous OAuth credentials")?;
                } else {
                    tokio::fs::remove_file(&path)
                        .await
                        .context("remove duplicate OAuth credentials")?;
                    match tokio::fs::remove_file(path.with_extension("lock")).await {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error).context("remove duplicate OAuth credential lock");
                        }
                    }
                }
                anyhow::ensure!(
                    !swapped,
                    "provider {} is bound to a different ChatGPT account; sign into that account again, or add the new account under another provider ID",
                    provider.id
                );
                anyhow::bail!(
                    "this ChatGPT account is already configured; sign into a different account, for example in a private browser window"
                );
            }
        }
        return Ok(());
    }
    let provider_id = provider.map_or(profile, |provider| provider.id.as_str());
    if !crate::config::load_stored_config()?.is_some_and(|config| {
        config
            .providers
            .iter()
            .any(|provider| provider.id == provider_id)
    }) {
        return Ok(());
    }
    crate::config::mutate_stored_config(|config| {
        for provider in config.providers.iter_mut().filter(|provider| {
            provider.preset_id.as_deref() == Some(profile)
                && provider.auth.api_key.trim().is_empty()
                && provider.id == provider_id
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
