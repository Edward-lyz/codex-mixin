use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Context;
use codex_mixin::config::GatewayConfig;
use codex_mixin::provider::capabilities::ProviderCapabilities;
use codex_mixin::provider::{
    AWS_BEDROCK_DEFAULT_REGION, AWS_BEDROCK_RUNTIME_SERVICE, AwsSigV4AuthConfig,
    MANUAL_MODEL_CONTEXT_WINDOW, ProviderModelSource, apply_discovered_models,
    aws_bedrock_runtime_base_url, discover_provider_models, redact_provider_error,
};
use serde_json::json;

use super::{
    TestProviderOptions, apply_baidu_auth_options, discovery::apply_discovered_quota,
    discovery::apply_inferred_custom_endpoint, discovery::detect_custom_provider_protocol,
    discovery::discover_custom_quota, discovery_settings_match, ensure_has_providers,
    find_provider_mut, mutate_and_invalidate, normalize_base_url, normalize_model_ids,
    required_config, trim_required,
};
use crate::cli::official_models::{
    OFFICIAL_PROVIDER_ID, available_official_ids, load_official_models, refresh_official_models,
};
use crate::cli::refresh_default_managed_codex_catalog;

pub(crate) async fn discover_models(id: &str) -> anyhow::Result<()> {
    discover_models_with_output(id, false).await
}

pub(crate) async fn discover_models_with_output(id: &str, quiet: bool) -> anyhow::Result<()> {
    if id == OFFICIAL_PROVIDER_ID {
        super::super::progress_step("Refreshing model list for provider official");
        let count = refresh_official_models().await?;
        super::super::progress_step(&format!(
            "Model refresh complete for official: {count} available"
        ));
        if !quiet {
            println!("provider models refreshed: official ({count} available)");
        }
        return Ok(());
    }
    let config = required_config()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?
        .clone();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    super::super::progress_step(&format!("Refreshing model list for provider {id}"));
    let quota_probe = async {
        if provider.preset_id.as_deref() == Some("custom") && provider.quota_url.is_none() {
            discover_custom_quota(&client, &provider).await
        } else {
            Ok(None)
        }
    };
    let (models, discovered_quota) =
        tokio::join!(discover_provider_models(&client, &provider), quota_probe);
    let discovered_quota = match discovered_quota {
        Ok(discovered) => discovered,
        Err(error) => {
            tracing::warn!(
                provider_id = provider.id,
                error = %redact_provider_error(&provider, &format!("{error:#}")),
                "custom quota discovery failed"
            );
            None
        }
    };
    let models = match models {
        Ok(models) => models,
        Err(error) => {
            let stored_error = redact_provider_error(&provider, &format!("{error:#}"));
            super::super::progress_step(&format!(
                "Model refresh failed for {id}: {}",
                stored_error
                    .lines()
                    .next()
                    .unwrap_or("model discovery failed")
            ));
            mutate_and_invalidate(|config| {
                let current = find_provider_mut(config, id)?;
                anyhow::ensure!(
                    discovery_settings_match(current, &provider),
                    "provider {id} discovery settings changed during refresh; retry"
                );
                current.models_refresh_error = Some(stored_error);
                if let Some(discovered_quota) = &discovered_quota {
                    apply_discovered_quota(current, discovered_quota);
                }
                Ok(())
            })?;
            return Err(error);
        }
    };
    super::super::progress_step(&format!(
        "Discovered {} models for provider {id}",
        models.len()
    ));
    let runtime_config = GatewayConfig::from_stored_config()?;
    let capabilities = ProviderCapabilities::from_default_path(&runtime_config)?;
    let mut annotated_provider = provider.clone();
    annotated_provider.cached_models = models;
    capabilities.annotate_provider(&mut annotated_provider);
    let models = annotated_provider.cached_models;
    let count = models.len();
    mutate_and_invalidate(|config| {
        let current = find_provider_mut(config, id)?;
        anyhow::ensure!(
            discovery_settings_match(current, &provider),
            "provider {id} discovery settings changed during refresh; retry"
        );
        if let Some(discovered_quota) = &discovered_quota {
            apply_discovered_quota(current, discovered_quota);
        }
        apply_discovered_models(current, models)
    })?;
    super::super::progress_step(&format!(
        "Model refresh complete for {id}: {count} available"
    ));
    if !quiet {
        println!("provider models refreshed: {id} ({count} available)");
        if let Some(discovered_quota) = discovered_quota {
            println!(
                "provider quota endpoint detected: {id} ({})",
                discovered_quota.url
            );
        }
    }
    Ok(())
}

pub(crate) async fn probe_selected_models(id: &str) -> anyhow::Result<()> {
    let config = required_config()?;
    let provider = config
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?
        .clone();
    let selected_models = provider
        .cached_models
        .iter()
        .filter(|model| {
            provider
                .selected_models
                .iter()
                .any(|selected| selected == &model.id)
        })
        .cloned()
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !selected_models.is_empty(),
        "provider {id} has no selected cached models; refresh the model list and select models first"
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    super::super::progress_step(&format!(
        "Probing {} selected models for provider {id}",
        selected_models.len()
    ));
    let provider_id = provider.id.clone();
    let summary = ProviderCapabilities::probe_provider_with_progress(
        client,
        &provider,
        &selected_models,
        Some(std::sync::Arc::new(move |done, total, supported, indeterminate| {
            super::super::progress_step(&format!(
                "Probing capabilities for {provider_id}: {done}/{total} complete ({supported} routed, {indeterminate} indeterminate)"
            ));
        })),
    )
    .await?;
    let runtime_config = GatewayConfig::from_stored_config()?;
    let mut capabilities = ProviderCapabilities::from_default_path(&runtime_config)?;
    capabilities.replace_provider_results(&provider, &runtime_config, &summary.results)?;
    mutate_and_invalidate(|config| {
        let current = find_provider_mut(config, id)?;
        anyhow::ensure!(
            discovery_settings_match(current, &provider),
            "provider {id} settings changed during capability probing; retry"
        );
        capabilities.annotate_provider(current);
        current.validate()
    })?;
    super::super::progress_step("Refreshing Codex model catalog after capability probing");
    refresh_default_managed_codex_catalog().await?;
    super::super::progress_step(&format!(
        "Capability probing complete for {id}: {} models checked",
        summary.attempted
    ));
    println!(
        "provider capabilities probed: {id} ({} models checked)",
        summary.attempted
    );
    Ok(())
}

pub(crate) async fn test_provider(options: TestProviderOptions) -> anyhow::Result<()> {
    let id = options.id.as_str();
    let config = required_config()?;
    let stored_provider = config
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
    let mut provider = stored_provider.clone();
    if let Some(key) = options.key {
        provider.auth.api_key = trim_required("key", key)?;
        provider.auth.aws_sigv4 = None;
    }
    let has_aws_override = options.aws_access_key_id.is_some()
        || options.aws_secret_access_key.is_some()
        || options.aws_session_token.is_some()
        || options.aws_region.is_some();
    if has_aws_override {
        anyhow::ensure!(
            provider.preset_id.as_deref() == Some("aws-bedrock"),
            "AWS credential options require an aws-bedrock provider"
        );
        let mut aws = provider
            .auth
            .aws_sigv4
            .take()
            .unwrap_or(AwsSigV4AuthConfig {
                access_key_id: String::new(),
                secret_access_key: String::new(),
                session_token: None,
                region: AWS_BEDROCK_DEFAULT_REGION.to_owned(),
                service: AWS_BEDROCK_RUNTIME_SERVICE.to_owned(),
            });
        if let Some(value) = options.aws_access_key_id {
            aws.access_key_id = trim_required("AWS access key ID", value)?;
        }
        if let Some(value) = options.aws_secret_access_key {
            aws.secret_access_key = trim_required("AWS secret access key", value)?;
        }
        if let Some(value) = options.aws_session_token {
            aws.session_token = Some(trim_required("AWS session token", value)?);
        }
        if let Some(value) = options.aws_region {
            aws.region = trim_required("AWS region", value)?;
            if options.base_url.is_none() {
                provider.base_url = aws_bedrock_runtime_base_url(&aws.region);
            }
        }
        provider.auth.api_key.clear();
        provider.auth.aws_sigv4 = Some(aws);
    }
    if let Some(base_url) = options.base_url {
        provider.base_url = normalize_base_url(base_url)?;
    }
    apply_baidu_auth_options(
        &mut provider,
        options.baidu_auth_bridge.as_deref(),
        options.ducx_executable,
    )?;
    if provider.preset_id.as_deref() == Some("custom")
        && !matches!(&provider.model_source, ProviderModelSource::Static)
    {
        let endpoint = detect_custom_provider_protocol(&provider)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("custom provider endpoint detection returned no result")
            })?;
        apply_inferred_custom_endpoint(&mut provider, endpoint);
    }
    provider.validate()?;
    let (mode, model_count) = match &provider.model_source {
        ProviderModelSource::Static => ("configuration", provider.cached_models.len()),
        _ => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;
            let models = discover_provider_models(&client, &provider)
                .await
                .with_context(|| {
                    format!(
                        "provider test failed for {id}; check the API key, base URL, and network"
                    )
                })?;
            ("models_endpoint", models.len())
        }
    };
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "provider_id": provider.id,
                "ok": true,
                "mode": mode,
                "model_count": model_count,
                "paid_inference_performed": false,
            }))?
        );
    } else if mode == "configuration" {
        println!(
            "provider test ok: {id} (static model source; configuration only, no paid inference)"
        );
    } else {
        println!("provider test ok: {id} ({model_count} models)");
    }
    Ok(())
}

pub(crate) fn select_models(
    id: &str,
    models: Vec<String>,
    model_contexts: Vec<String>,
) -> anyhow::Result<()> {
    let models = normalize_model_ids(models)?;
    let model_contexts = parse_model_contexts(model_contexts)?;
    let selected_count = models.len();
    if id == OFFICIAL_PROVIDER_ID {
        anyhow::ensure!(
            model_contexts.is_empty(),
            "official model context windows cannot be overridden"
        );
        let available_models = load_official_models()?;
        let available_ids = available_official_ids(&available_models);
        for model in &models {
            anyhow::ensure!(
                available_ids.contains(model.as_str()),
                "official provider has no known model {model}; refresh the OpenAI model list first"
            );
        }
        mutate_and_invalidate(|config| {
            config.official_selected_models = Some(models);
            Ok(())
        })?;
        println!("provider models selected: {id} ({selected_count})");
        return Ok(());
    }
    mutate_and_invalidate(|config| {
        ensure_has_providers(config)?;
        apply_model_selection(find_provider_mut(config, id)?, models, &model_contexts)
    })?;
    println!("provider models selected: {id} ({selected_count})");
    Ok(())
}

pub(super) fn apply_model_selection(
    provider: &mut codex_mixin::provider::ProviderDefinition,
    models: Vec<String>,
    model_contexts: &BTreeMap<String, u64>,
) -> anyhow::Result<()> {
    let mut known = provider
        .cached_models
        .iter()
        .map(|model| model.id.clone())
        .collect::<std::collections::HashSet<_>>();
    for model in &models {
        if known.insert(model.clone()) {
            provider
                .cached_models
                .push(codex_mixin::provider::ProviderModel {
                    id: model.clone(),
                    manually_added: true,
                    context_window: Some(MANUAL_MODEL_CONTEXT_WINDOW),
                    supports_image: Some(false),
                    supports_thinking: Some(true),
                    supports_web_search: Some(false),
                    supports_tool_search: Some(false),
                    supports_function_tools: Some(true),
                    ..codex_mixin::provider::ProviderModel::default()
                });
        }
    }
    let selected = models
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    for (model_id, context_window) in model_contexts {
        anyhow::ensure!(
            selected.contains(model_id.as_str()),
            "model context override targets unselected model {model_id}"
        );
        let model = provider
            .cached_models
            .iter_mut()
            .find(|model| model.id == *model_id)
            .ok_or_else(|| anyhow::anyhow!("unknown model context override: {model_id}"))?;
        anyhow::ensure!(
            model.manually_added,
            "model context can only be edited for manually added models: {model_id}"
        );
        model.context_window = Some(*context_window);
    }
    provider
        .cached_models
        .retain(|model| !model.manually_added || selected.contains(model.id.as_str()));
    provider.selected_models = models;
    provider.new_models.clear();
    provider.validate()
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
