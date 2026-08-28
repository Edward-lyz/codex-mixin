mod probe;
mod storage;
mod types;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use futures_util::{StreamExt, stream};
use reqwest::Client;

use crate::config::GatewayConfig;
use crate::provider::{ProviderDefinition, ProviderModel, ProviderModelSource, ProviderRegistry};

use storage::{default_capability_path, load_file, unix_milliseconds, update_file};
use types::{
    CapabilityFile, PROBE_CONCURRENCY, PROBE_REQUEST_CONCURRENCY, ProviderCapabilityRecord,
};
pub use types::{
    CapabilityStatus, ModelCapabilities, ProtocolCapabilities, ProviderIdentity,
    ProviderProbeSummary,
};

type ProbeProgress = Arc<dyn Fn(usize, usize, usize, usize) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct ProviderCapabilities {
    path: PathBuf,
    file: CapabilityFile,
}

impl ProviderCapabilities {
    pub fn from_default_path(config: &GatewayConfig) -> anyhow::Result<Self> {
        Self::load(default_capability_path(), config)
    }

    pub fn load(path: PathBuf, config: &GatewayConfig) -> anyhow::Result<Self> {
        let mut file = load_file(&path)?;
        file.providers.retain(|provider_id, record| {
            config
                .providers
                .iter()
                .find(|provider| provider.id == *provider_id)
                .is_some_and(|provider| {
                    record.identity == ProviderIdentity::from_provider(provider)
                })
        });
        Ok(Self { path, file })
    }

    pub fn annotate_config(&self, config: &mut GatewayConfig) {
        for provider in &mut config.providers {
            let Some(record) = self.file.providers.get(&provider.id) else {
                continue;
            };
            if record.identity != ProviderIdentity::from_provider(provider) {
                continue;
            }
            annotate_models(&mut provider.cached_models, &record.models);
        }
    }

    pub fn annotate_provider(&self, provider: &mut ProviderDefinition) {
        let Some(record) = self.file.providers.get(&provider.id) else {
            return;
        };
        if record.identity == ProviderIdentity::from_provider(provider) {
            annotate_models(&mut provider.cached_models, &record.models);
        }
    }

    pub async fn probe_provider(
        client: Client,
        provider: &ProviderDefinition,
        models: &[ProviderModel],
    ) -> anyhow::Result<ProviderProbeSummary> {
        Self::probe_provider_with_progress(client, provider, models, None).await
    }

    pub async fn probe_provider_with_progress(
        client: Client,
        provider: &ProviderDefinition,
        models: &[ProviderModel],
        progress: Option<ProbeProgress>,
    ) -> anyhow::Result<ProviderProbeSummary> {
        let mut definition = provider.clone();
        definition.cached_models = models.to_vec();
        let registry = ProviderRegistry::new(vec![definition])?;
        let runtime = Arc::new(
            registry
                .provider(&provider.id)
                .context("capability probe provider is missing from registry")?
                .clone(),
        );
        let native_headers = if provider.model_source == ProviderModelSource::BaiduOneApi
            && runtime.uses_ducx_loopback()
        {
            Some(crate::provider::native_baidu_headers(&runtime).await?)
        } else {
            None
        };
        let probed_at_ms = unix_milliseconds()?;
        let request_limit = Arc::new(tokio::sync::Semaphore::new(PROBE_REQUEST_CONCURRENCY));
        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let supported = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let indeterminate = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let total = models.len();
        let mut results = stream::iter(models.iter().map(|model| {
            let client = client.clone();
            let runtime = Arc::clone(&runtime);
            let native_headers = native_headers.clone();
            let request_limit = Arc::clone(&request_limit);
            let completed = Arc::clone(&completed);
            let supported = Arc::clone(&supported);
            let indeterminate = Arc::clone(&indeterminate);
            let progress = progress.clone();
            let model_id = model.id.clone();
            async move {
                let result = probe::probe_model(
                    &client,
                    &runtime,
                    &model_id,
                    probed_at_ms,
                    &request_limit,
                    native_headers.as_ref(),
                )
                .await;
                let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if result.selected().is_some() {
                    supported.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    indeterminate.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                if let Some(progress) = progress {
                    progress(
                        done,
                        total,
                        supported.load(std::sync::atomic::Ordering::Relaxed),
                        indeterminate.load(std::sync::atomic::Ordering::Relaxed),
                    );
                }
                result
            }
        }))
        .buffer_unordered(PROBE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        results.sort_by(|left, right| left.model.cmp(&right.model));
        Ok(ProviderProbeSummary {
            attempted: results.len(),
            supported: results
                .iter()
                .filter(|result| result.selected().is_some())
                .count(),
            indeterminate: results
                .iter()
                .filter(|result| result.selected().is_none())
                .count(),
            results,
        })
    }

    pub fn replace_provider_results(
        &mut self,
        expected_provider: &ProviderDefinition,
        current_config: &GatewayConfig,
        results: &[ModelCapabilities],
    ) -> anyhow::Result<()> {
        let current = current_config
            .providers
            .iter()
            .find(|provider| provider.id == expected_provider.id)
            .context("provider was removed while capability probing was running")?;
        let expected_identity = ProviderIdentity::from_provider(expected_provider);
        anyhow::ensure!(
            ProviderIdentity::from_provider(current) == expected_identity,
            "provider changed while capability probing was running"
        );
        let provider_id = expected_provider.id.clone();
        let results = results.to_vec();
        self.file = update_file(&self.path, |file| {
            let previous_models = file
                .providers
                .get(&provider_id)
                .filter(|record| record.identity == expected_identity)
                .map(|record| &record.models);
            let models = results
                .iter()
                .map(|result| {
                    let result = previous_models
                        .and_then(|models| models.get(&result.model))
                        .map_or_else(
                            || result.clone(),
                            |previous| merge_model_result(previous, result.clone()),
                        );
                    (result.model.clone(), result)
                })
                .collect();
            file.providers.insert(
                provider_id.clone(),
                ProviderCapabilityRecord {
                    identity: expected_identity.clone(),
                    models,
                },
            );
            Ok(())
        })?;
        Ok(())
    }

    pub fn clear_default_cache() -> anyhow::Result<bool> {
        let path = default_capability_path();
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        Ok(true)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn annotate_models(
    models: &mut [ProviderModel],
    capabilities: &BTreeMap<String, ModelCapabilities>,
) {
    for model in models {
        let Some(capability) = capabilities.get(&model.id) else {
            continue;
        };
        model.protocol = capability.selected_protocol;
        model.api_path = capability.selected_api_path.clone();
        model.capabilities_probed_at_ms = Some(capability.probed_at_ms);
        model.capability_probe_error = capability.last_probe_error.clone();
        let Some(selected) = capability.selected() else {
            if model.capability_probe_error.is_none() {
                model.capability_probe_error = capability
                    .protocols
                    .iter()
                    .filter_map(|protocol| protocol.error.as_deref())
                    .next()
                    .map(str::to_owned);
            }
            continue;
        };
        model.supports_image = selected.image_input.as_option_bool();
        model.supports_thinking = selected
            .thinking
            .as_option_bool()
            .or(model.supports_thinking);
        model.supports_function_tools = selected.function_tools.as_option_bool();
        model.supports_tool_search = selected.tool_search.as_option_bool();
        model.supports_web_search = selected.web_search.as_option_bool();
        if model.capability_probe_error.is_none() {
            model.capability_probe_error = selected.error.clone();
        }
    }
}

fn merge_model_result(
    previous: &ModelCapabilities,
    mut current: ModelCapabilities,
) -> ModelCapabilities {
    let errors = current
        .protocols
        .iter()
        .filter_map(|protocol| protocol.error.as_deref())
        .collect::<Vec<_>>();
    current.last_probe_error = (!errors.is_empty()).then(|| errors.join("; "));
    for protocol in &mut current.protocols {
        let Some(old) = previous
            .protocols
            .iter()
            .find(|old| old.protocol == protocol.protocol && old.api_path == protocol.api_path)
        else {
            continue;
        };
        if protocol.baseline == CapabilityStatus::Indeterminate {
            protocol.baseline = old.baseline;
        }
        if protocol.image_input == CapabilityStatus::Indeterminate {
            protocol.image_input = old.image_input;
        }
        if protocol.thinking == CapabilityStatus::Indeterminate {
            protocol.thinking = old.thinking;
        }
        if protocol.function_tools == CapabilityStatus::Indeterminate {
            protocol.function_tools = old.function_tools;
        }
        if protocol.tool_search == CapabilityStatus::Indeterminate {
            protocol.tool_search = old.tool_search;
        }
        if protocol.web_search == CapabilityStatus::Indeterminate {
            protocol.web_search = old.web_search;
        }
    }
    if current.selected_protocol.is_none() && previous.selected_protocol.is_some() {
        current.selected_protocol = previous.selected_protocol;
        current.selected_api_path = previous.selected_api_path.clone();
        for old in &previous.protocols {
            if !current.protocols.iter().any(|protocol| {
                protocol.protocol == old.protocol && protocol.api_path == old.api_path
            }) {
                current.protocols.push(old.clone());
            }
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protocol_capabilities(tool_search: CapabilityStatus) -> ProtocolCapabilities {
        ProtocolCapabilities {
            protocol: crate::provider::ProviderProtocol::OpenAiResponses,
            api_path: "/v1/responses".to_owned(),
            baseline: CapabilityStatus::Supported,
            image_input: CapabilityStatus::Supported,
            thinking: CapabilityStatus::Supported,
            function_tools: CapabilityStatus::Supported,
            tool_search,
            web_search: CapabilityStatus::Unsupported,
            error: (tool_search == CapabilityStatus::Indeterminate)
                .then(|| "rate limited".to_owned()),
        }
    }

    #[test]
    fn transient_probe_preserves_last_known_capability() {
        let previous = ModelCapabilities {
            model: "model-a".to_owned(),
            selected_protocol: Some(crate::provider::ProviderProtocol::OpenAiResponses),
            selected_api_path: Some("/v1/responses".to_owned()),
            protocols: vec![protocol_capabilities(CapabilityStatus::Supported)],
            probed_at_ms: 1,
            last_probe_error: None,
        };
        let current = ModelCapabilities {
            model: "model-a".to_owned(),
            selected_protocol: Some(crate::provider::ProviderProtocol::OpenAiResponses),
            selected_api_path: Some("/v1/responses".to_owned()),
            protocols: vec![protocol_capabilities(CapabilityStatus::Indeterminate)],
            probed_at_ms: 2,
            last_probe_error: None,
        };

        let merged = merge_model_result(&previous, current);

        assert_eq!(
            merged.selected().unwrap().tool_search,
            CapabilityStatus::Supported
        );
        assert_eq!(merged.last_probe_error.as_deref(), Some("rate limited"));
        assert_eq!(merged.probed_at_ms, 2);
    }

    #[test]
    fn indeterminate_thinking_probe_preserves_advertised_support() {
        let mut protocol = protocol_capabilities(CapabilityStatus::Supported);
        protocol.thinking = CapabilityStatus::Indeterminate;
        let capabilities = ModelCapabilities {
            model: "model-a".to_owned(),
            selected_protocol: Some(crate::provider::ProviderProtocol::OpenAiResponses),
            selected_api_path: Some("/v1/responses".to_owned()),
            protocols: vec![protocol],
            probed_at_ms: 2,
            last_probe_error: None,
        };
        let model = crate::provider::ProviderModel {
            id: "model-a".to_owned(),
            supports_thinking: Some(true),
            ..Default::default()
        };

        let mut models = [model];
        annotate_models(
            &mut models,
            &BTreeMap::from([("model-a".to_owned(), capabilities)]),
        );

        assert_eq!(models[0].supports_thinking, Some(true));
    }
}
