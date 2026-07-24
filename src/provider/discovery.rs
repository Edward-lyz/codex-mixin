use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use reqwest::Client;
use serde_json::json;

use crate::anthropic::{BaiduAvailableModelsResponse, ModelsResponse};

use super::{ProviderDefinition, ProviderModel, ProviderModelSource, ProviderRuntime};

pub async fn discover_provider_models(
    client: &Client,
    definition: &ProviderDefinition,
) -> anyhow::Result<Vec<ProviderModel>> {
    let provider = ProviderRuntime::new(definition.clone())?;
    match &definition.model_source {
        ProviderModelSource::Static => Ok(definition.cached_models.clone()),
        ProviderModelSource::OpenAiCompatible { .. } => {
            let url = provider
                .models_url()
                .context("provider models URL is not configured")?
                .clone();
            let response = provider.apply_auth(client.get(url)).send().await?;
            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                anyhow::bail!(
                    "provider {} models endpoint returned {status}: {body}",
                    provider.id()
                );
            }
            let models: ModelsResponse = serde_json::from_str(&body)
                .context("provider models endpoint returned invalid JSON")?;
            let mut models = models
                .data
                .into_iter()
                .map(|model| ProviderModel {
                    id: model.id,
                    display_name: model.display_name,
                    description: model.description,
                    ratio: model.ratio,
                    price_type: model.price_type,
                    context_window: model.context_window,
                    supports_image: model.supports_image,
                    supports_thinking: model.supports_thinking,
                    supports_web_search: model.supports_web_search,
                })
                .collect::<Vec<_>>();
            normalize_models(&mut models);
            Ok(models)
        }
        ProviderModelSource::BaiduOneApi => {
            let url = provider
                .models_url()
                .context("provider available-models URL is not configured")?
                .clone();
            let response = provider
                .apply_auth(client.post(url))
                .json(&json!({}))
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                anyhow::bail!(
                    "provider {} available-models endpoint returned {status}: {body}",
                    provider.id()
                );
            }
            let available: BaiduAvailableModelsResponse = serde_json::from_str(&body)
                .context("provider available-models endpoint returned invalid JSON")?;
            if !available.success {
                anyhow::bail!(
                    "provider {} available-models endpoint failed: {}",
                    provider.id(),
                    available.message
                );
            }
            let mut models = Vec::with_capacity(available.data.len());
            let mut model_indices = HashMap::with_capacity(available.data.len());
            for model in available.data {
                let (id, is_internal) = match model.model.strip_suffix("-内部") {
                    Some(canonical) => (canonical.to_owned(), true),
                    None => (model.model.clone(), false),
                };
                let Some(capability) = model.capability else {
                    tracing::warn!(
                        provider_id = provider.id(),
                        model = %model.model,
                        "excluding available-models entry without capability metadata"
                    );
                    continue;
                };
                let description = capability.model_description;
                let converted = ProviderModel {
                    id: id.clone(),
                    display_name: Some(description.clone()),
                    description: Some(description),
                    ratio: Some(capability.ratio),
                    price_type: Some(model.price_type),
                    context_window: Some(capability.context_window),
                    supports_image: Some(capability.supports_image),
                    supports_thinking: Some(capability.supports_thinking),
                    supports_web_search: None,
                };
                if let Some(&index) = model_indices.get(&id) {
                    if !is_internal {
                        models[index] = converted;
                    }
                } else {
                    model_indices.insert(id, models.len());
                    models.push(converted);
                }
            }
            normalize_models(&mut models);
            Ok(models)
        }
    }
}

pub fn apply_discovered_models(
    provider: &mut ProviderDefinition,
    models: Vec<ProviderModel>,
) -> anyhow::Result<()> {
    let first_successful_refresh = provider.models_refreshed_at_ms.is_none();
    if first_successful_refresh {
        provider.selected_models = models.iter().map(|model| model.id.clone()).collect();
        provider.new_models.clear();
    } else {
        let previous_models = provider
            .cached_models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<HashSet<_>>();
        let selected_models = provider
            .selected_models
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let previous_new_models = provider
            .new_models
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        provider.new_models = models
            .iter()
            .filter(|model| {
                !selected_models.contains(model.id.as_str())
                    && (previous_new_models.contains(model.id.as_str())
                        || !previous_models.contains(model.id.as_str()))
            })
            .map(|model| model.id.clone())
            .collect();
    }
    provider.cached_models = models;
    provider.models_refreshed_at_ms =
        Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64);
    provider.models_refresh_error = None;
    provider.validate()
}

pub fn redact_provider_error(definition: &ProviderDefinition, error: &str) -> String {
    let redacted = if definition.auth.api_key.is_empty() {
        error.to_owned()
    } else {
        error.replace(&definition.auth.api_key, "<redacted>")
    };
    redacted.chars().take(8_000).collect()
}

fn normalize_models(models: &mut Vec<ProviderModel>) {
    models.retain(|model| !model.id.trim().is_empty());
    models.sort_by(|left, right| {
        left.id
            .to_ascii_lowercase()
            .cmp(&right.id.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    models.dedup_by(|left, right| left.id == right.id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> ProviderModel {
        ProviderModel {
            id: id.to_owned(),
            ..ProviderModel::default()
        }
    }

    #[test]
    fn first_refresh_selects_every_model_without_marking_them_new() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();

        apply_discovered_models(&mut provider, vec![model("a"), model("b")]).unwrap();

        assert_eq!(provider.selected_models, ["a", "b"]);
        assert!(provider.new_models.is_empty());
    }

    #[test]
    fn later_refresh_marks_only_new_unselected_models_and_retains_unavailable_selection() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();
        provider.models_refreshed_at_ms = Some(1);
        provider.cached_models = vec![model("a"), model("gone")];
        provider.selected_models = vec!["a".to_owned(), "gone".to_owned()];

        apply_discovered_models(&mut provider, vec![model("a"), model("new")]).unwrap();

        assert_eq!(provider.selected_models, ["a", "gone"]);
        assert_eq!(provider.new_models, ["new"]);
        assert_eq!(
            provider
                .cached_models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "new"]
        );
    }
}
