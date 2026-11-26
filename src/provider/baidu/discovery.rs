//! Baidu OneAPI available-models discovery.

use std::collections::HashMap;

use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use super::super::discovery::{enrich_from_models_dev, normalize_models};
use super::super::resolver::enrich_models_with_models_dev;
use super::super::{ProviderDefinition, ProviderModel, ProviderRuntime};

/// Response of the Baidu OneAPI `/openapi/v2/available_models` endpoint.
#[derive(Debug, Deserialize)]
pub(crate) struct BaiduAvailableModelsResponse {
    pub data: Vec<BaiduAvailableModel>,
    pub success: bool,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BaiduAvailableModel {
    pub model: String,
    pub capability: Option<BaiduModelCapability>,
    #[serde(default)]
    pub capability_set: Vec<String>,
    pub price_type: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BaiduModelCapability {
    pub supports_image: bool,
    pub context_window: u64,
    pub ratio: String,
    pub model_description: String,
}

pub(crate) async fn discover_baidu_models(
    client: &Client,
    definition: &ProviderDefinition,
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
    enrich_from_models_dev(
        client,
        definition,
        &mut models,
        enrich_models_with_models_dev,
    )
    .await;
    normalize_models(&mut models);
    Ok(models)
}

fn add_baidu_model(
    provider: &ProviderRuntime,
    model: BaiduAvailableModel,
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
        display_name: Some(id.clone()),
        description: Some(description),
        ratio: Some(capability.ratio),
        price_type: Some(model.price_type),
        context_window: Some(capability.context_window),
        supports_image: Some(capability.supports_image || declared_capability("image")),
        supports_thinking: Some(true),
        // The endpoint currently ships an empty capability_set, so an absent
        // token means "unknown", not "unsupported". The catalog is never
        // probed; earlier web-search probe results still apply.
        supports_web_search: declared_capability("web_search").then_some(true),
        supports_tool_search: Some(false),
        supports_function_tools: Some(true),
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
