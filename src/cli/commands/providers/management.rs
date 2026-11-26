use codex_mixin::application::provider::after_provider_commit;
use codex_mixin::application::provider::discovery::{
    apply_inferred_custom_endpoint, detect_custom_provider_protocol,
};
use codex_mixin::provider::ProviderModelSource;

use codex_mixin::application::error::OperationError;

use codex_mixin::application::provider::build::{AddProviderInput, UpdateProviderInput};

use crate::cli::commands::providers::{discover_models_with_output, sync_imagegen_skill};

pub(crate) async fn add_provider(options: AddProviderInput) -> anyhow::Result<()> {
    let mut shutdown = codex_mixin::platform::ShutdownSignal::install()?;
    tokio::select! {
        result = add_provider_inner(options) => result,
        _ = shutdown.recv() => anyhow::bail!("provider addition cancelled; refresh to check any completed steps"),
    }
}

async fn add_provider_inner(options: AddProviderInput) -> anyhow::Result<()> {
    let preset = codex_mixin::provider::ProviderPreset::parse(&options.preset)?;
    let timeout = options
        .oauth_timeout_seconds
        .unwrap_or(codex_mixin::provider::OAUTH_LOGIN_TIMEOUT_SECONDS);
    if preset == codex_mixin::provider::ProviderPreset::Official {
        anyhow::ensure!(
            options
                .key
                .as_deref()
                .is_none_or(|key| key.trim().is_empty())
                && options.base_url.is_none()
                && options.protocol.is_none()
                && options.api_path.is_none()
                && options.aws_access_key_id.is_none()
                && options.aws_secret_access_key.is_none()
                && options.aws_session_token.is_none()
                && options.models_path.is_none()
                && options.header_env.is_empty(),
            "the OpenAI OAuth preset uses the official account and endpoint"
        );
        if options
            .id
            .as_deref()
            .is_none_or(|id| matches!(id, "official" | "openai"))
        {
            codex_mixin::application::provider::login::authorize_add(
                preset,
                None,
                timeout,
                |url| {
                    crate::cli::progress_step(&format!(
                        "Open this URL to authorize, or cancel: {url}"
                    ));
                },
            )
            .await?;
            discover_models_with_output("official", false)
                .await
                .map_err(|source| OperationError::AfterCommit {
                    stage: "OAuth model discovery",
                    source,
                })?;
            println!("provider added: official");
            return Ok(());
        }
    }
    // A usable local AGY login is reused; the browser only opens when none
    // exists or the user asked to sign in again.
    let browser_oauth = matches!(
        preset,
        codex_mixin::provider::ProviderPreset::Official
            | codex_mixin::provider::ProviderPreset::Agy
            | codex_mixin::provider::ProviderPreset::AgyBusiness
    ) && options
        .key
        .as_deref()
        .is_none_or(|key| key.trim().is_empty())
        && (preset == codex_mixin::provider::ProviderPreset::Official
            || options
                .id
                .as_deref()
                .is_some_and(|id| id != preset.as_str())
            || options.reauthorize
            || !codex_mixin::application::provider::local_agy_login_available(preset));
    let reauthorize = options.reauthorize;
    let mut prepared = codex_mixin::application::provider::build::create_provider(options)?;
    if browser_oauth {
        let existing = codex_mixin::config::load_stored_config()?
            .unwrap_or_default()
            .providers
            .into_iter()
            .find(|provider| provider.id == prepared.id);
        if reauthorize && let Some(existing) = existing.as_ref() {
            anyhow::ensure!(
                existing.preset_id.as_deref() == Some(preset.as_str()),
                "provider {} uses a different preset",
                prepared.id
            );
            codex_mixin::application::provider::login::authorize_add(
                preset,
                Some(existing),
                timeout,
                |url| {
                    crate::cli::progress_step(&format!(
                        "Open this URL to authorize, or cancel: {url}"
                    ));
                },
            )
            .await?;
            discover_models_with_output(&prepared.id, false)
                .await
                .map_err(|source| OperationError::AfterCommit {
                    stage: "OAuth model discovery",
                    source,
                })?;
            println!("provider authorized: {}", prepared.id);
            return Ok(());
        }
        anyhow::ensure!(
            !reauthorize || existing.is_some() || prepared.provider.is_legacy_oauth_instance(),
            "provider {} not found; add it without --reauthorize",
            prepared.id
        );
        anyhow::ensure!(
            existing.is_none(),
            "provider {} already exists; choose a different provider ID",
            prepared.id
        );
        codex_mixin::application::provider::login::authorize_add(
            preset,
            Some(&prepared.provider),
            timeout,
            |url| {
                crate::cli::progress_step(&format!("Open this URL to authorize, or cancel: {url}"));
            },
        )
        .await?;
    }
    let mut detected_protocol = None;
    let mut protocol_probe_error = None;
    if prepared.should_probe_protocol {
        match detect_custom_provider_protocol(&prepared.provider).await {
            Ok(Some(endpoint)) => {
                detected_protocol = Some(
                    crate::cli::commands::providers::protocol_name(endpoint.protocol).to_owned(),
                );
                apply_inferred_custom_endpoint(&mut prepared.provider, endpoint);
            }
            Ok(None) => {}
            Err(error) => protocol_probe_error = Some(error),
        }
    }
    let configured_protocol =
        crate::cli::commands::providers::protocol_name(prepared.provider.protocol);
    let fresh_credential = browser_oauth
        .then(|| {
            (!prepared.provider.is_legacy_oauth_instance())
                .then(|| prepared.provider.owned_oauth_path())
                .flatten()
        })
        .flatten();
    if let Err(error) = codex_mixin::application::provider::add_provider(
        prepared.provider,
        prepared.gateway_api_key,
    ) {
        // The browser login wrote a credential for an instance that was never saved.
        if let (Some(path), OperationError::BeforeCommit { .. }) = (fresh_credential, &error) {
            for file in [path.clone(), path.with_extension("lock")] {
                if let Err(cleanup) = std::fs::remove_file(&file)
                    && cleanup.kind() != std::io::ErrorKind::NotFound
                {
                    eprintln!(
                        "failed to remove unsaved OAuth credential {}: {cleanup}",
                        file.display()
                    );
                }
            }
        }
        return Err(error.into());
    }
    after_provider_commit("imagegen skill sync", sync_imagegen_skill)?;
    println!("provider added: {}", prepared.id);
    if let Some(protocol) = detected_protocol {
        println!("provider protocol detected: {} ({protocol})", prepared.id);
    }
    if let Some(error) = protocol_probe_error {
        eprintln!(
            "provider protocol detection failed for {}; keeping {}: {error:#}",
            prepared.id, configured_protocol
        );
    }
    let changes = match discover_models_with_output(&prepared.id, false).await {
        Ok(changes) => changes,
        Err(source) => {
            return Err(OperationError::AfterCommit {
                stage: "model discovery",
                source,
            }
            .into());
        }
    };
    if !changes.auto_selected.is_empty() {
        crate::cli::commands::providers::models::probe_new_models(
            &prepared.id,
            &changes.auto_selected,
            true,
        )
        .await
        .map_err(|source| OperationError::AfterCommit {
            stage: "model capability probe",
            source,
        })?;
    }
    Ok(())
}

pub(crate) async fn update_provider(options: UpdateProviderInput) -> anyhow::Result<()> {
    let mut prepared =
        codex_mixin::application::provider::build::update_provider_from_input(options)?;
    let id = prepared.id.clone();
    let mut detected_protocol = None;
    let mut protocol_probe_error = None;
    if prepared.should_probe_protocol {
        match detect_custom_provider_protocol(&prepared.provider).await {
            Ok(Some(endpoint)) => {
                detected_protocol = Some(
                    crate::cli::commands::providers::protocol_name(endpoint.protocol).to_owned(),
                );
                codex_mixin::application::provider::commit_detected_endpoint(
                    &id,
                    &prepared.provider,
                    endpoint.base_url,
                    endpoint.protocol,
                    endpoint.api_path,
                    endpoint.models_path,
                )?;
                prepared.provider = codex_mixin::application::provider::provider_for_refresh(&id)?;
            }
            Ok(None) => {}
            Err(error) => protocol_probe_error = Some(error),
        }
    }
    after_provider_commit("imagegen skill sync", sync_imagegen_skill)?;
    println!("provider updated: {id}");
    if let Some(protocol) = detected_protocol {
        println!("provider protocol detected: {id} ({protocol})");
    }
    if let Some(error) = protocol_probe_error {
        eprintln!(
            "provider protocol detection failed for {id}; keeping the configured endpoint: {error:#}"
        );
    }
    if prepared.should_refresh_capabilities
        && prepared.provider.model_source != ProviderModelSource::BaiduOneApi
    {
        discover_models_with_output(&id, false)
            .await
            .map_err(|source| OperationError::AfterCommit {
                stage: "model discovery",
                source,
            })?;
    }
    Ok(())
}

pub(crate) fn set_provider_enabled(id: &str, enabled: bool) -> anyhow::Result<()> {
    codex_mixin::application::provider::set_provider_enabled(id, enabled)?;
    after_provider_commit("imagegen skill sync", sync_imagegen_skill)?;
    println!(
        "provider {}: {id}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

pub(crate) fn remove_provider(id: &str) -> anyhow::Result<()> {
    let change = codex_mixin::application::provider::remove_provider(id)?;
    after_provider_commit("imagegen skill sync", sync_imagegen_skill)?;
    println!("provider removed: {id}");
    for (old_id, new_id) in change.renames {
        println!("provider renumbered: {old_id} -> {new_id}");
    }
    Ok(())
}

pub(crate) fn reorder_providers(ids: Vec<String>) -> anyhow::Result<()> {
    let order = ids.join(", ");
    codex_mixin::application::provider::reorder_providers(&ids)?;
    println!("provider order updated: {order}");
    Ok(())
}
