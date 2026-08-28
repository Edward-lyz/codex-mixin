use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use reqwest::Client;
use serde_json::json;

use crate::anthropic::{BaiduAvailableModelsResponse, ModelInfo, ModelsResponse};

use super::models_dev::{
    enrich_models_with_models_dev, fetch_models_dev_provider_models, uses_models_dev_capabilities,
};
use super::{ProviderDefinition, ProviderModel, ProviderModelSource, ProviderRuntime};

pub async fn discover_provider_models(
    client: &Client,
    definition: &ProviderDefinition,
) -> anyhow::Result<Vec<ProviderModel>> {
    let provider = ProviderRuntime::new(definition.clone(), &|name| std::env::var(name).ok())?;
    let native_headers = native_baidu_headers_if_needed(definition, &provider).await?;
    match &definition.model_source {
        ProviderModelSource::Static => Ok(definition.cached_models.clone()),
        ProviderModelSource::OpenAiCompatible { .. } => {
            discover_openai_models(client, definition, &provider).await
        }
        ProviderModelSource::BaiduOneApi => {
            discover_baidu_models(client, &provider, native_headers.as_ref()).await
        }
    }
}

async fn native_baidu_headers_if_needed(
    definition: &ProviderDefinition,
    provider: &ProviderRuntime,
) -> anyhow::Result<Option<reqwest::header::HeaderMap>> {
    if definition.model_source == ProviderModelSource::BaiduOneApi && provider.uses_ducx_loopback()
    {
        Ok(Some(super::native_baidu_headers(provider).await?))
    } else {
        Ok(None)
    }
}

async fn discover_openai_models(
    client: &Client,
    definition: &ProviderDefinition,
    provider: &ProviderRuntime,
) -> anyhow::Result<Vec<ProviderModel>> {
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
    let models: ModelsResponse =
        serde_json::from_str(&body).context("provider models endpoint returned invalid JSON")?;
    let mut models = models
        .data
        .into_iter()
        .map(|model| {
            openai_model_to_provider_model(
                model,
                definition.preset_id.as_deref() == Some("openrouter"),
            )
        })
        .collect::<Vec<_>>();
    if uses_models_dev_capabilities(definition.preset_id.as_deref(), definition.id.as_str()) {
        match fetch_models_dev_provider_models(client, "opencode-go").await {
            Ok(metadata) => enrich_models_with_models_dev(&mut models, &metadata),
            Err(error) => tracing::warn!(
                provider_id = %definition.id,
                error = %format!("{error:#}"),
                "failed to enrich OpenCode Go models from models.dev"
            ),
        }
    }
    normalize_models(&mut models);
    Ok(models)
}

fn openai_model_to_provider_model(model: ModelInfo, is_openrouter: bool) -> ProviderModel {
    let (supports_image, supports_thinking, supports_web_search, supports_function_tools) =
        if is_openrouter {
            let supports_parameter = |parameter| {
                model
                    .supported_parameters
                    .iter()
                    .any(|supported| supported == parameter)
            };
            (
                Some(model.architecture.as_ref().is_some_and(|architecture| {
                    architecture
                        .input_modalities
                        .iter()
                        .any(|modality| modality == "image")
                })),
                Some(
                    model
                        .reasoning
                        .as_ref()
                        .is_some_and(|reasoning| !reasoning.is_null())
                        || supports_parameter("reasoning"),
                ),
                Some(supports_parameter("web_search_options")),
                Some(supports_parameter("tools") && supports_parameter("tool_choice")),
            )
        } else {
            (Some(false), None, Some(false), Some(false))
        };
    ProviderModel {
        id: model.id,
        display_name: model.display_name,
        description: model.description,
        ratio: model.ratio,
        price_type: model.price_type,
        context_window: model.context_window,
        protocol: model.protocol,
        api_path: model.api_path,
        supports_image,
        supports_thinking,
        supports_web_search,
        supports_tool_search: Some(false),
        supports_function_tools,
        capability_probe_error: model.capability_probe_error,
        capabilities_probed_at_ms: model.capabilities_probed_at_ms,
    }
}

async fn discover_baidu_models(
    client: &Client,
    provider: &ProviderRuntime,
    native_headers: Option<&reqwest::header::HeaderMap>,
) -> anyhow::Result<Vec<ProviderModel>> {
    let url = provider
        .models_url()
        .context("provider available-models URL is not configured")?
        .clone();
    let request = match native_headers {
        Some(headers) => client.post(url).headers(headers.clone()),
        None => provider.apply_auth(client.post(url)),
    };
    let response = request.json(&json!({})).send().await?;
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
        add_baidu_model(provider, model, &mut models, &mut model_indices);
    }
    normalize_models(&mut models);
    Ok(models)
}

fn add_baidu_model(
    provider: &ProviderRuntime,
    model: crate::anthropic::BaiduAvailableModel,
    models: &mut Vec<ProviderModel>,
    model_indices: &mut HashMap<String, usize>,
) {
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
        return;
    };
    let declared_capability = |name| {
        model
            .capability_set
            .iter()
            .any(|capability| capability == name)
    };
    let description = capability.model_description;
    let converted = ProviderModel {
        id: id.clone(),
        display_name: Some(description.clone()),
        description: Some(description),
        ratio: Some(capability.ratio),
        price_type: Some(model.price_type),
        context_window: Some(capability.context_window),
        supports_image: Some(capability.supports_image || declared_capability("image")),
        supports_thinking: Some(capability.supports_thinking || declared_capability("thinking")),
        supports_web_search: Some(declared_capability("web_search")),
        supports_tool_search: Some(false),
        supports_function_tools: Some(false),
        ..ProviderModel::default()
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
        let available_models = models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<HashSet<_>>();
        provider
            .selected_models
            .retain(|model| available_models.contains(model.as_str()));
        // Refreshes preserve explicit user selection. New models remain visible for
        // review, while models removed upstream leave the selection immediately.
        provider.new_models = models
            .iter()
            .filter(|model| !previous_models.contains(model.id.as_str()))
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
    fn openrouter_declarations_populate_capabilities_without_a_probe() {
        let response: ModelsResponse = serde_json::from_str(
            r#"{"data":[{"id":"vision-tool-model","name":"Vision Tool","context_length":128000,"architecture":{"input_modalities":["text","image"]},"supported_parameters":["tools","tool_choice","reasoning","web_search_options"],"reasoning":{}}]}"#,
        )
        .unwrap();

        let model = openai_model_to_provider_model(response.data.into_iter().next().unwrap(), true);

        assert_eq!(model.display_name.as_deref(), Some("Vision Tool"));
        assert_eq!(model.context_window, Some(128000));
        assert_eq!(model.supports_image, Some(true));
        assert_eq!(model.supports_thinking, Some(true));
        assert_eq!(model.supports_function_tools, Some(true));
        assert_eq!(model.supports_web_search, Some(true));
        assert_eq!(model.supports_tool_search, Some(false));
    }

    #[test]
    fn undeclared_openai_compatible_thinking_is_unknown() {
        let response: ModelsResponse =
            serde_json::from_str(r#"{"data":[{"id":"unknown"}]}"#).unwrap();

        let model =
            openai_model_to_provider_model(response.data.into_iter().next().unwrap(), false);

        assert_eq!(model.supports_image, Some(false));
        assert_eq!(model.supports_thinking, None);
        assert_eq!(model.supports_function_tools, Some(false));
        assert_eq!(model.supports_web_search, Some(false));
        assert_eq!(model.supports_tool_search, Some(false));
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
    fn later_refresh_keeps_new_models_unselected_and_removes_unavailable_models() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();
        provider.models_refreshed_at_ms = Some(1);
        provider.cached_models = vec![model("a"), model("gone")];
        provider.selected_models = vec!["a".to_owned(), "gone".to_owned()];

        apply_discovered_models(&mut provider, vec![model("a"), model("new")]).unwrap();

        assert_eq!(provider.selected_models, ["a"]);
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

    #[test]
    fn reappearing_model_stays_unselected_after_leaving_cached_models() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();
        provider.models_refreshed_at_ms = Some(1);
        provider.cached_models = vec![model("flap")];
        provider.selected_models = vec!["flap".to_owned()];

        // User deselects "flap" manually; apply_model_selection clears new_models.
        provider.selected_models.clear();

        // Upstream drops "flap" entirely.
        apply_discovered_models(&mut provider, vec![model("other")]).unwrap();
        assert!(provider.selected_models.is_empty());
        assert_eq!(provider.new_models, ["other"]);

        apply_discovered_models(&mut provider, vec![model("flap"), model("other")]).unwrap();
        assert!(provider.selected_models.is_empty());
        assert_eq!(provider.new_models, ["flap"]);
    }

    #[test]
    fn discovery_then_manual_select_then_rediscover_does_not_dup_or_retag() {
        // The first discovery keeps the bootstrap behavior for a newly configured provider.
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();

        apply_discovered_models(&mut provider, vec![model("a"), model("new")]).unwrap();
        assert_eq!(provider.selected_models, ["a", "new"]);
        assert!(provider.new_models.is_empty());

        // Simulate apply_model_selection clearing new_models after a manual save
        // that keeps the same selection.
        provider.new_models.clear();

        apply_discovered_models(&mut provider, vec![model("a"), model("new")]).unwrap();
        assert_eq!(provider.selected_models, ["a", "new"]);
        assert!(provider.new_models.is_empty());
    }
}
