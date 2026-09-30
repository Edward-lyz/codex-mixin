use std::collections::BTreeMap;

use anyhow::Context;
use codex_mixin::catalog::apply_official_context_overrides;
use codex_mixin::provider::{ModelDiscoveryChanges, redact_provider_error};
use serde_json::json;

use super::{TestProviderOptions, mutate_and_invalidate, normalize_model_ids};
use crate::cli::official_models::{
    OFFICIAL_PROVIDER_ID, available_official_ids, load_official_catalog, load_official_models,
    refresh_official_models,
};
use crate::cli::refresh_default_managed_codex_catalog;

pub(crate) async fn discover_models(id: &str) -> anyhow::Result<()> {
    let changes = discover_models_with_output(id, false).await?;
    if id == OFFICIAL_PROVIDER_ID {
        if !changes.added.is_empty() || !changes.removed.is_empty() {
            refresh_default_managed_codex_catalog().await?;
            crate::cli::sync_installed_client_models()?;
        }
        return Ok(());
    }
    if !changes.auto_selected.is_empty() {
        probe_new_models(id, &changes.auto_selected, true).await?;
    }
    Ok(())
}

pub(crate) async fn discover_models_with_output(
    id: &str,
    quiet: bool,
) -> anyhow::Result<ModelDiscoveryChanges> {
    if id == OFFICIAL_PROVIDER_ID {
        super::super::progress_step("Refreshing model list for provider official");
        let previous_models = load_official_models()?;
        let count = refresh_official_models().await?;
        let current_models = load_official_models()?;
        let changes = apply_official_model_refresh(&previous_models, &current_models)?;
        super::super::progress_step(&format!(
            "Model refresh complete for official: {count} available"
        ));
        if !quiet {
            println!("provider models refreshed: official ({count} available)");
        }
        return Ok(changes);
    }
    super::super::progress_step(&format!("Refreshing model list for provider {id}"));
    let refreshed =
        match codex_mixin::application::provider::discovery::refresh_provider_models(id).await {
            Ok(refreshed) => refreshed,
            Err(error) => {
                if let Ok(provider) = codex_mixin::application::provider::provider_for_refresh(id) {
                    let safe_error = redact_provider_error(&provider, &format!("{error:#}"));
                    super::super::progress_step(&format!(
                        "Model refresh failed for {id}: {}",
                        safe_error
                            .lines()
                            .next()
                            .unwrap_or("model discovery failed")
                    ));
                }
                return Err(error);
            }
        };
    super::super::progress_step(&format!(
        "Model refresh complete for {id}: {} available",
        refreshed.model_count
    ));
    if !quiet {
        println!(
            "provider models refreshed: {id} ({} available)",
            refreshed.model_count
        );
        if let Some(url) = refreshed.quota_url {
            println!("provider quota endpoint detected: {id} ({url})");
        }
    }
    Ok(refreshed.changes)
}

pub(in crate::cli) fn apply_official_model_refresh(
    previous_models: &[codex_mixin::provider::ProviderModel],
    current_models: &[codex_mixin::provider::ProviderModel],
) -> anyhow::Result<ModelDiscoveryChanges> {
    if previous_models.is_empty() {
        return Ok(ModelDiscoveryChanges::default());
    }
    let previous_ids = previous_models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let current_ids = current_models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut changes = ModelDiscoveryChanges {
        added: current_models
            .iter()
            .filter(|model| !previous_ids.contains(model.id.as_str()))
            .map(|model| model.id.clone())
            .collect(),
        removed: previous_models
            .iter()
            .filter(|model| !current_ids.contains(model.id.as_str()))
            .map(|model| model.id.clone())
            .collect(),
        auto_selected: Vec::new(),
    };
    if changes.added.is_empty() && changes.removed.is_empty() {
        return Ok(changes);
    }
    let auto_select = !changes.added.is_empty()
        && changes.added.len() < codex_mixin::provider::AUTO_SELECT_NEW_MODEL_LIMIT;
    mutate_and_invalidate(|config| {
        if let Some(selected) = &mut config.official_selected_models {
            selected.retain(|model| current_ids.contains(model.as_str()));
            if auto_select {
                selected.extend(changes.added.iter().cloned());
            }
        } else if !auto_select && !changes.added.is_empty() {
            config.official_selected_models = Some(
                previous_models
                    .iter()
                    .filter(|model| current_ids.contains(model.id.as_str()))
                    .map(|model| model.id.clone())
                    .collect(),
            );
        }
        Ok(())
    })?;
    if auto_select {
        changes.auto_selected = changes.added.clone();
    }
    Ok(changes)
}

pub(crate) async fn probe_selected_models(id: &str) -> anyhow::Result<()> {
    probe_models(id, None, false, false, true).await
}

/// Probe newly added or selected models automatically. Probes are paid
/// completions, so a disabled provider is skipped; its selected models are
/// probed by the gateway once the provider is enabled again.
pub(crate) async fn probe_new_models(
    id: &str,
    model_ids: &[String],
    refresh_clients: bool,
) -> anyhow::Result<()> {
    let enabled = super::required_config()?
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?
        .enabled;
    if !enabled {
        tracing::info!(
            provider_id = id,
            models = model_ids.join(","),
            "skipping automatic capability probe for a disabled provider"
        );
        super::super::progress_step(&format!(
            "Skipping capability probing for disabled provider {id}"
        ));
        return Ok(());
    }
    probe_models(id, Some(model_ids), true, true, refresh_clients).await
}

async fn probe_models(
    id: &str,
    model_ids: Option<&[String]>,
    quiet: bool,
    fallback_only: bool,
    refresh_clients: bool,
) -> anyhow::Result<()> {
    let summary = codex_mixin::application::provider::models::probe_provider_models(
        id,
        model_ids,
        fallback_only,
        refresh_clients,
    )
    .await?;
    if refresh_clients && summary.attempted > 0 {
        super::super::progress_step("Refreshing Codex model catalog after capability probing");
        refresh_default_managed_codex_catalog().await?;
        let refreshed_clients = crate::cli::sync_installed_client_models()?;
        for client in &refreshed_clients {
            super::super::progress_step(&format!(
                "{client} models refreshed; restart {client} to reload"
            ));
        }
    }
    super::super::progress_step(&format!(
        "Capability probing complete for {id}: {} models checked",
        summary.attempted
    ));
    if !quiet {
        println!(
            "provider capabilities probed: {id} ({} models checked)",
            summary.attempted
        );
    }
    Ok(())
}

pub(crate) async fn test_provider(options: TestProviderOptions) -> anyhow::Result<()> {
    let id = options.id.clone();
    let result = codex_mixin::application::provider::models::test_provider(
        codex_mixin::application::provider::models::TestProviderInput {
            id: options.id,
            key: options.key,
            aws_access_key_id: options.aws_access_key_id,
            aws_secret_access_key: options.aws_secret_access_key,
            aws_session_token: options.aws_session_token,
            aws_region: options.aws_region,
            base_url: options.base_url,
            baidu_auth_bridge: options.baidu_auth_bridge,
            ducx_executable: options.ducx_executable,
        },
    )
    .await?;
    let mode = result.mode;
    let model_count = result.model_count;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider_id": id,
                "ok": true,
                "mode": match mode {
                    codex_mixin::application::provider::models::ProviderTestMode::Configuration => "configuration",
                    codex_mixin::application::provider::models::ProviderTestMode::ModelsEndpoint => "models_endpoint",
                },
                "model_count": model_count,
                "paid_inference_performed": false,
            }))?
        );
    } else if mode == codex_mixin::application::provider::models::ProviderTestMode::Configuration {
        println!(
            "provider test ok: {id} (static model source; configuration only, no paid inference)"
        );
    } else {
        println!("provider test ok: {id} ({model_count} models)");
    }
    Ok(())
}

pub(crate) async fn select_models(
    id: &str,
    models: Vec<String>,
    model_contexts: Vec<String>,
    clear_model_contexts: Vec<String>,
) -> anyhow::Result<()> {
    let models = normalize_model_ids(models)?;
    let model_contexts = parse_model_contexts(model_contexts)?;
    let clear_model_contexts = normalize_model_ids(clear_model_contexts)?;
    let selected_count = models.len();
    for model in &clear_model_contexts {
        anyhow::ensure!(
            !model_contexts.contains_key(model),
            "model context override is both set and cleared: {model}"
        );
    }
    if id == OFFICIAL_PROVIDER_ID {
        let available_models = load_official_models()?;
        let available_ids = available_official_ids(&available_models);
        for model in &models {
            anyhow::ensure!(
                available_ids.contains(model.as_str()),
                "official provider has no known model {model}; refresh the OpenAI model list first"
            );
        }
        for model in model_contexts.keys() {
            anyhow::ensure!(
                available_ids.contains(model.as_str()),
                "official provider has no known model {model}; refresh the OpenAI model list first"
            );
        }
        if !model_contexts.is_empty() {
            let mut catalog = load_official_catalog()?.context(
                "official model catalog is missing; refresh the OpenAI model list first",
            )?;
            apply_official_context_overrides(&mut catalog, &model_contexts)?;
        }
        let context_only =
            models.is_empty() && (!model_contexts.is_empty() || !clear_model_contexts.is_empty());
        mutate_and_invalidate(|config| {
            if !context_only {
                config.official_selected_models = Some(models);
            }
            config.official_model_contexts.extend(model_contexts);
            for model in clear_model_contexts {
                config.official_model_contexts.remove(&model);
            }
            Ok(())
        })?;
        if context_only {
            println!("official model context windows updated");
        } else {
            println!("provider models selected: {id} ({selected_count})");
        }
        return Ok(());
    }
    let context_only =
        models.is_empty() && (!model_contexts.is_empty() || !clear_model_contexts.is_empty());
    let models_to_probe = codex_mixin::application::provider::models::select_provider_models(
        id,
        models,
        model_contexts,
        clear_model_contexts,
    )?;
    if !models_to_probe.is_empty() {
        probe_new_models(id, &models_to_probe, true).await?;
    }
    if context_only {
        println!("provider model context windows updated: {id}");
    } else {
        println!("provider models selected: {id} ({selected_count})");
    }
    Ok(())
}

fn parse_model_contexts(values: Vec<String>) -> anyhow::Result<BTreeMap<String, u64>> {
    let mut contexts = BTreeMap::new();
    for value in values {
        let (model_id, context_window) = value
            .rsplit_once('=')
            .ok_or_else(|| anyhow::anyhow!("model context must use MODEL=TOKENS: {value}"))?;
        let model_id = super::trim_required("model context model", model_id.to_owned())?;
        let context_window = context_window.trim().parse::<u64>().with_context(|| {
            format!("invalid context window for model {model_id}: {context_window}")
        })?;
        anyhow::ensure!(
            context_window > 0,
            "model context window must be greater than zero: {model_id}"
        );
        anyhow::ensure!(
            contexts.insert(model_id.clone(), context_window).is_none(),
            "duplicate model context override: {model_id}"
        );
    }
    Ok(contexts)
}

#[cfg(test)]
mod tests {
    use super::parse_model_contexts;

    #[test]
    fn rejects_duplicate_model_context_overrides() {
        let error = parse_model_contexts(vec![
            "model-a=128000".to_owned(),
            "model-a=256000".to_owned(),
        ])
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("duplicate model context override")
        );
    }
}
