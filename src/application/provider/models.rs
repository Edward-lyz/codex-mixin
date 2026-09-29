use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;

use crate::config::{GatewayConfig, mutate_stored_config};
use crate::provider::capabilities::{ProviderCapabilities, ProviderProbeSummary};
use crate::provider::{
    AWS_BEDROCK_DEFAULT_REGION, AWS_BEDROCK_RUNTIME_SERVICE, AwsSigV4AuthConfig,
    ModelDiscoveryChanges, ProviderDefinition, ProviderModel, ProviderModelSource,
    apply_discovered_models, aws_bedrock_runtime_base_url, discover_provider_models,
};

use super::super::error::OperationError;
use super::{
    after_provider_commit, commit_provider_change, provider_for_refresh, provider_mut,
    require_providers, set_auxiliary_upstream,
};

pub fn apply_model_selection(
    provider: &mut ProviderDefinition,
    models: Vec<String>,
    model_contexts: &BTreeMap<String, u64>,
    clear_model_contexts: &[String],
) -> anyhow::Result<Vec<String>> {
    let previous_selection = provider
        .selected_models
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut known = provider
        .cached_models
        .iter()
        .map(|model| model.id.clone())
        .collect::<HashSet<_>>();
    let models_to_probe = models
        .iter()
        .filter(|model| {
            !previous_selection.contains(model.as_str()) || !known.contains(model.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    for model in &models {
        if known.insert(model.clone()) {
            provider.cached_models.push(ProviderModel {
                id: model.clone(),
                manually_added: true,
                context_window: Some(crate::provider::MANUAL_MODEL_CONTEXT_WINDOW),
                ..ProviderModel::default()
            });
        }
    }
    for model_id in clear_model_contexts {
        provider.model_context_overrides.remove(model_id);
        if let Some(model) = provider
            .cached_models
            .iter_mut()
            .find(|model| model.id == *model_id)
        {
            model.context_window = model.source_context_window;
        }
    }
    for (model_id, context_window) in model_contexts {
        let model = provider
            .cached_models
            .iter_mut()
            .find(|model| model.id == *model_id)
            .ok_or_else(|| anyhow::anyhow!("unknown model context override: {model_id}"))?;
        if model.source_context_window.is_none() {
            model.source_context_window = model.context_window;
        }
        model.context_window = Some(*context_window);
        provider
            .model_context_overrides
            .insert(model_id.clone(), *context_window);
    }
    let selected = models.iter().map(String::as_str).collect::<HashSet<_>>();
    provider
        .cached_models
        .retain(|model| !model.manually_added || selected.contains(model.id.as_str()));
    let available = provider
        .cached_models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<HashSet<_>>();
    provider
        .model_context_overrides
        .retain(|model, _| available.contains(model.as_str()));
    provider.selected_models = models;
    provider.new_models.clear();
    provider.prune_stale_auto_review_model();
    provider.validate()?;
    Ok(models_to_probe)
}

pub fn select_provider_models(
    id: &str,
    models: Vec<String>,
    model_contexts: BTreeMap<String, u64>,
    clear_model_contexts: Vec<String>,
) -> Result<Vec<String>, OperationError> {
    for model in &clear_model_contexts {
        if model_contexts.contains_key(model) {
            return Err(OperationError::BeforeCommit {
                source: anyhow::anyhow!("model context override is both set and cleared: {model}"),
            });
        }
    }
    commit_provider_change(|config| {
        require_providers(config)?;
        let provider = provider_mut(config, id)?;
        let context_only =
            models.is_empty() && (!model_contexts.is_empty() || !clear_model_contexts.is_empty());
        let selection = if context_only {
            provider.selected_models.clone()
        } else {
            models
        };
        apply_model_selection(provider, selection, &model_contexts, &clear_model_contexts)
    })
    .and_then(|models_to_probe| {
        super::invalidate_web_search_cache()?;
        Ok(models_to_probe)
    })
}

pub async fn probe_provider_models(
    id: &str,
    requested_models: Option<&[String]>,
    fallback_only: bool,
    replace_results: bool,
) -> anyhow::Result<ProviderProbeSummary> {
    let provider = provider_for_refresh(id)?;
    let mut models = provider
        .cached_models
        .iter()
        .filter(|model| {
            requested_models.map_or_else(
                || {
                    provider
                        .selected_models
                        .iter()
                        .any(|selected| selected == &model.id)
                },
                |requested| requested.iter().any(|requested| requested == &model.id),
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !models.is_empty(),
        "provider {id} has no requested cached models to probe"
    );
    if fallback_only {
        let config = GatewayConfig::from_stored_config()?;
        let capabilities = ProviderCapabilities::from_default_path(&config)?;
        let needing = capabilities
            .models_needing_probe(&provider, &models)?
            .into_iter()
            .collect::<HashSet<_>>();
        models.retain(|model| needing.contains(&model.id));
        if models.is_empty() {
            return Ok(ProviderProbeSummary::default());
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let summary = ProviderCapabilities::probe_provider(client, &provider, &models).await?;
    let config = GatewayConfig::from_stored_config()?;
    let mut capabilities = ProviderCapabilities::from_default_path(&config)?;
    if replace_results {
        capabilities.replace_provider_results(&provider, &config, &summary.results)?;
    } else {
        capabilities.merge_provider_results(&provider, &config, &summary.results)?;
    }
    mutate_stored_config(|stored| {
        let current = provider_mut(stored, id)?;
        anyhow::ensure!(
            current.base_url == provider.base_url
                && current.model_source == provider.model_source
                && current.auth == provider.auth,
            "provider {id} settings changed during capability probing; retry"
        );
        capabilities.annotate_provider(current);
        current.validate()
    })?;
    super::invalidate_web_search_cache()?;
    after_provider_commit("provider capability cache invalidation", || {
        ProviderCapabilities::clear_default_cache().map(|_| ())
    })?;
    Ok(summary)
}

#[derive(Clone, Debug, Default)]
pub struct TestProviderInput {
    pub id: String,
    pub key: Option<String>,
    pub aws_access_key_id: Option<String>,
    pub aws_secret_access_key: Option<String>,
    pub aws_session_token: Option<String>,
    pub aws_region: Option<String>,
    pub base_url: Option<String>,
    pub baidu_auth_bridge: Option<String>,
    pub ducx_executable: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderTestMode {
    Configuration,
    ModelsEndpoint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderTestResult {
    pub mode: ProviderTestMode,
    pub model_count: usize,
}

pub async fn test_provider(input: TestProviderInput) -> anyhow::Result<ProviderTestResult> {
    let config = crate::config::load_stored_config()?
        .ok_or_else(|| anyhow::anyhow!("provider configuration is missing"))?;
    let mut provider = config
        .providers
        .into_iter()
        .find(|provider| provider.id == input.id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {}", input.id))?;
    if let Some(key) = input.key {
        provider.auth.api_key = required("key", key)?;
        provider.auth.aws_sigv4 = None;
    }
    let has_aws_override = input.aws_access_key_id.is_some()
        || input.aws_secret_access_key.is_some()
        || input.aws_session_token.is_some()
        || input.aws_region.is_some();
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
        if let Some(value) = input.aws_access_key_id {
            aws.access_key_id = required("AWS access key ID", value)?;
        }
        if let Some(value) = input.aws_secret_access_key {
            aws.secret_access_key = required("AWS secret access key", value)?;
        }
        if let Some(value) = input.aws_session_token {
            aws.session_token = Some(required("AWS session token", value)?);
        }
        if let Some(value) = input.aws_region {
            aws.region = required("AWS region", value)?;
            if input.base_url.is_none() {
                provider.base_url = aws_bedrock_runtime_base_url(&aws.region);
            }
        }
        provider.auth.api_key.clear();
        provider.auth.aws_sigv4 = Some(aws);
    }
    if let Some(base_url) = input.base_url {
        provider.base_url = normalize_base_url(base_url)?;
    }
    if let Some(bridge) = input.baidu_auth_bridge.as_deref() {
        provider.request_policy.baidu_auth_bridge = Some(match bridge {
            "disabled" => crate::provider::BaiduAuthBridge::Disabled,
            "ducx_loopback" => crate::provider::BaiduAuthBridge::DucxLoopback,
            other => anyhow::bail!(
                "invalid Baidu auth bridge {other}; expected disabled or ducx_loopback"
            ),
        });
    }
    if let Some(executable) = input.ducx_executable {
        provider.request_policy.ducx_executable = Some(executable);
    }
    if provider.preset_id.as_deref() == Some("custom")
        && !matches!(provider.model_source, ProviderModelSource::Static)
    {
        let endpoint = super::discovery::detect_custom_provider_protocol(&provider)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("custom provider endpoint detection returned no result")
            })?;
        super::discovery::apply_inferred_custom_endpoint(&mut provider, endpoint);
    }
    provider.validate()?;
    let (mode, model_count) = match provider.model_source {
        ProviderModelSource::Static => (
            ProviderTestMode::Configuration,
            provider.cached_models.len(),
        ),
        _ => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;
            let models = discover_provider_models(&client, &provider)
                .await
                .with_context(|| {
                    format!(
                        "provider test failed for {}; check the API key, base URL, and network",
                        provider.id
                    )
                })?;
            (ProviderTestMode::ModelsEndpoint, models.len())
        }
    };
    Ok(ProviderTestResult { mode, model_count })
}

fn required(label: &str, value: String) -> anyhow::Result<String> {
    let value = value.trim().to_owned();
    anyhow::ensure!(!value.is_empty(), "{label} cannot be empty");
    Ok(value)
}

fn normalize_base_url(value: String) -> anyhow::Result<String> {
    let mut value = required("base URL", value)?;
    while value.ends_with('/') {
        value.pop();
    }
    anyhow::ensure!(
        value.starts_with("http://") || value.starts_with("https://"),
        "base URL must start with http:// or https://"
    );
    Ok(value)
}

pub fn apply_discovered_provider_models(
    provider: &mut ProviderDefinition,
    models: Vec<ProviderModel>,
) -> anyhow::Result<ModelDiscoveryChanges> {
    apply_discovered_models(provider, models)
}

pub fn set_provider_auxiliary_model_upstream(
    id: &str,
    enabled: bool,
) -> Result<(), OperationError> {
    commit_provider_change(|config| set_auxiliary_upstream(config, id, enabled))?;
    super::invalidate_provider_caches()
}
