use std::collections::HashMap;

use anyhow::{Context, anyhow};
use reqwest::Client;
use serde::Deserialize;

use super::{ProviderModel, OPEN_CODE_GO_PRESET_ID};

pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";

#[derive(Debug, Deserialize)]
struct ModelsDevCatalog {
    #[serde(flatten)]
    providers: HashMap<String, ModelsDevProvider>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    #[serde(default)]
    models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    attachment: Option<bool>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    modalities: Option<ModelsDevModalities>,
    #[serde(default)]
    limit: Option<ModelsDevLimit>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModalities {
    #[serde(default)]
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevLimit {
    #[serde(default)]
    context: Option<u64>,
    #[serde(default)]
    input: Option<u64>,
}

/// Fetch provider model capability metadata from models.dev.
///
/// OpenCode Go's `/v1/models` endpoint typically returns IDs only. OpenCode itself
/// loads full limits/capabilities from models.dev, so codex-mixin should do the same
/// for that provider instead of inventing a local capability table.
pub async fn fetch_models_dev_provider_models(
    client: &Client,
    provider_id: &str,
) -> anyhow::Result<HashMap<String, ProviderModel>> {
    let response = client
        .get(MODELS_DEV_API_URL)
        .header(reqwest::header::USER_AGENT, "codex-mixin")
        .send()
        .await
        .with_context(|| format!("failed to request models.dev catalog for {provider_id}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read models.dev catalog body")?;
    if !status.is_success() {
        return Err(anyhow!(
            "models.dev catalog returned {status} for {provider_id}: {body}"
        ));
    }
    parse_models_dev_provider_models(provider_id, &body)
}

pub fn parse_models_dev_provider_models(
    provider_id: &str,
    body: &str,
) -> anyhow::Result<HashMap<String, ProviderModel>> {
    let catalog: ModelsDevCatalog =
        serde_json::from_str(body).context("models.dev catalog returned invalid JSON")?;
    let Some(provider) = catalog.providers.get(provider_id) else {
        return Err(anyhow!(
            "models.dev catalog does not include provider {provider_id}"
        ));
    };

    let mut models = HashMap::with_capacity(provider.models.len());
    for (model_id, model) in &provider.models {
        let id = model_id.trim();
        if id.is_empty() {
            continue;
        }
        models.insert(
            id.to_owned(),
            ProviderModel {
                id: id.to_owned(),
                display_name: model.name.clone(),
                description: model.description.clone(),
                context_window: model
                    .limit
                    .as_ref()
                    .and_then(|limit| limit.context.or(limit.input)),
                supports_image: Some(model_supports_image(model)),
                supports_thinking: model.reasoning,
                ..ProviderModel::default()
            },
        );
    }
    Ok(models)
}

pub fn enrich_models_with_models_dev(
    models: &mut [ProviderModel],
    metadata: &HashMap<String, ProviderModel>,
) {
    for model in models {
        let Some(source) = metadata.get(model.id.as_str()) else {
            continue;
        };
        // models.dev is the capability SSOT for OpenCode Go. Prefer its values over
        // empty/ID-only discovery payloads.
        if let Some(display_name) = source.display_name.clone() {
            model.display_name = Some(display_name);
        }
        if let Some(description) = source.description.clone() {
            model.description = Some(description);
        }
        if let Some(context_window) = source.context_window {
            model.context_window = Some(context_window);
        }
        if let Some(supports_image) = source.supports_image {
            model.supports_image = Some(supports_image);
        }
        if let Some(supports_thinking) = source.supports_thinking {
            model.supports_thinking = Some(supports_thinking);
        }
    }
}

pub fn uses_models_dev_capabilities(definition_preset_id: Option<&str>, provider_id: &str) -> bool {
    definition_preset_id == Some(OPEN_CODE_GO_PRESET_ID) || provider_id == OPEN_CODE_GO_PRESET_ID
}

fn model_supports_image(model: &ModelsDevModel) -> bool {
    if model.attachment == Some(true) {
        return true;
    }
    model.modalities.as_ref().is_some_and(|modalities| {
        modalities
            .input
            .iter()
            .any(|modality| modality.eq_ignore_ascii_case("image"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencode_go_capabilities_from_models_dev_payload() {
        let body = r#"{
          "opencode-go": {
            "id": "opencode-go",
            "models": {
              "gpt-5.6-luna": {
                "id": "gpt-5.6-luna",
                "name": "GPT-5.6 Luna (2x usage)",
                "description": "Cost-efficient GPT-5.6 model",
                "attachment": true,
                "reasoning": true,
                "modalities": {"input": ["text", "image", "pdf"], "output": ["text"]},
                "limit": {"context": 1050000, "input": 922000, "output": 128000}
              },
              "glm-5.2": {
                "name": "GLM-5.2",
                "attachment": false,
                "reasoning": true,
                "limit": {"context": 1000000, "output": 131072}
              }
            }
          }
        }"#;

        let models = parse_models_dev_provider_models("opencode-go", body).unwrap();
        let luna = models.get("gpt-5.6-luna").unwrap();
        assert_eq!(luna.display_name.as_deref(), Some("GPT-5.6 Luna (2x usage)"));
        assert_eq!(luna.context_window, Some(1_050_000));
        assert_eq!(luna.supports_image, Some(true));
        assert_eq!(luna.supports_thinking, Some(true));

        let glm = models.get("glm-5.2").unwrap();
        assert_eq!(glm.context_window, Some(1_000_000));
        assert_eq!(glm.supports_image, Some(false));
        assert_eq!(glm.supports_thinking, Some(true));
    }

    #[test]
    fn enrich_prefers_models_dev_fields_over_id_only_discovery() {
        let metadata = HashMap::from([(
            "gpt-5.6-luna".to_owned(),
            ProviderModel {
                id: "gpt-5.6-luna".to_owned(),
                display_name: Some("GPT-5.6 Luna (2x usage)".to_owned()),
                description: Some("from models.dev".to_owned()),
                context_window: Some(1_050_000),
                supports_image: Some(true),
                supports_thinking: Some(true),
                ..ProviderModel::default()
            },
        )]);
        let mut models = vec![ProviderModel {
            id: "gpt-5.6-luna".to_owned(),
            ..ProviderModel::default()
        }];

        enrich_models_with_models_dev(&mut models, &metadata);

        assert_eq!(
            models[0].display_name.as_deref(),
            Some("GPT-5.6 Luna (2x usage)")
        );
        assert_eq!(models[0].description.as_deref(), Some("from models.dev"));
        assert_eq!(models[0].context_window, Some(1_050_000));
        assert_eq!(models[0].supports_image, Some(true));
        assert_eq!(models[0].supports_thinking, Some(true));
    }
}
