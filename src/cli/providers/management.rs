use codex_mixin::application::provider::after_provider_commit;
use codex_mixin::provider::ProviderModelSource;

use codex_mixin::application::error::OperationError;

use super::{
    AddProviderOptions, UpdateProviderOptions, discover_models_with_output,
    discovery::{apply_inferred_custom_endpoint, detect_custom_provider_protocol},
    sync_imagegen_skill,
};

pub(crate) async fn add_provider(options: AddProviderOptions) -> anyhow::Result<()> {
    let mut prepared = codex_mixin::application::provider::build::create_provider(options)?;
    let mut detected_protocol = None;
    let mut protocol_probe_error = None;
    if prepared.should_probe_protocol {
        match detect_custom_provider_protocol(&prepared.provider).await {
            Ok(Some(endpoint)) => {
                detected_protocol = Some(super::protocol_name(endpoint.protocol).to_owned());
                apply_inferred_custom_endpoint(&mut prepared.provider, endpoint);
            }
            Ok(None) => {}
            Err(error) => protocol_probe_error = Some(error),
        }
    }
    let configured_protocol = super::protocol_name(prepared.provider.protocol);
    codex_mixin::application::provider::add_provider(prepared.provider, prepared.gateway_api_key)?;
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
        super::models::probe_new_models(&prepared.id, &changes.auto_selected, true)
            .await
            .map_err(|source| OperationError::AfterCommit {
                stage: "model capability probe",
                source,
            })?;
    }
    Ok(())
}

pub(crate) async fn update_provider(options: UpdateProviderOptions) -> anyhow::Result<()> {
    let mut prepared =
        codex_mixin::application::provider::build::update_provider_from_input(options)?;
    let id = prepared.id.clone();
    let mut detected_protocol = None;
    let mut protocol_probe_error = None;
    if prepared.should_probe_protocol {
        match detect_custom_provider_protocol(&prepared.provider).await {
            Ok(Some(endpoint)) => {
                detected_protocol = Some(super::protocol_name(endpoint.protocol).to_owned());
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
