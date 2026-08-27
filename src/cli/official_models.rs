use std::collections::HashSet;

use codex_mixin::catalog::load_template_catalog;
use codex_mixin::config::GatewayConfig;
use codex_mixin::provider::{ProviderModel, ProviderProtocol};
use serde_json::Value;

pub(super) const OFFICIAL_PROVIDER_ID: &str = "official";

pub(super) fn load_official_models() -> anyhow::Result<Vec<ProviderModel>> {
    load_template_catalog(None)?.map_or_else(
        || Ok(Vec::new()),
        |catalog| official_models_from_catalog(&catalog),
    )
}

pub(super) fn selected_official_models(
    config: &GatewayConfig,
) -> anyhow::Result<Vec<ProviderModel>> {
    if !config.accept_codex_oauth || !config.codex_auth_path.is_file() {
        return Ok(Vec::new());
    }
    let models = load_official_models()?;
    Ok(filter_selected_models(
        models,
        config.official_selected_models.as_deref(),
    ))
}

pub(super) fn filter_official_catalog(
    catalog: &mut Value,
    selected_models: Option<&[String]>,
) -> anyhow::Result<()> {
    let Some(selected_models) = selected_models else {
        return Ok(());
    };
    let selected_models = selected_models
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let models = catalog
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("official Codex catalog has no models array"))?;
    models.retain(|model| {
        model
            .get("slug")
            .and_then(Value::as_str)
            .is_some_and(|slug| selected_models.contains(slug))
    });
    Ok(())
}

pub(super) fn available_official_ids(models: &[ProviderModel]) -> HashSet<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

fn filter_selected_models(
    models: Vec<ProviderModel>,
    selected_models: Option<&[String]>,
) -> Vec<ProviderModel> {
    let Some(selected_models) = selected_models else {
        return models;
    };
    let selected_models = selected_models
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    models
        .into_iter()
        .filter(|model| selected_models.contains(model.id.as_str()))
        .collect()
}

fn official_models_from_catalog(catalog: &Value) -> anyhow::Result<Vec<ProviderModel>> {
    let models = catalog
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("official Codex catalog has no models array"))?;
    models
        .iter()
        .filter(|model| model.get("visibility").and_then(Value::as_str) != Some("hide"))
        .map(|model| {
            let id = model
                .get("slug")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("official Codex model is missing slug"))?;
            let supports_image = model
                .get("input_modalities")
                .and_then(Value::as_array)
                .is_some_and(|modalities| {
                    modalities
                        .iter()
                        .any(|modality| modality.as_str() == Some("image"))
                });
            let supports_thinking = model
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .is_some_and(|levels| !levels.is_empty());
            let supports_tool_search = model
                .get("experimental_supported_tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| {
                    tools
                        .iter()
                        .any(|tool| tool.as_str() == Some("tool_search"))
                });
            Ok(ProviderModel {
                id: id.to_owned(),
                display_name: model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                description: model
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                context_window: model
                    .get("context_window")
                    .and_then(Value::as_u64)
                    .or_else(|| model.get("max_context_window").and_then(Value::as_u64)),
                protocol: Some(ProviderProtocol::OpenAiResponses),
                api_path: Some("/responses".to_owned()),
                supports_image: Some(supports_image),
                supports_thinking: Some(supports_thinking),
                supports_web_search: Some(
                    model.get("supports_search_tool").and_then(Value::as_bool) == Some(true),
                ),
                supports_tool_search: Some(supports_tool_search),
                supports_function_tools: Some(true),
                ..ProviderModel::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn converts_and_filters_official_catalog_models() {
        let catalog = json!({"models":[
            {
                "slug":"gpt-5.6-sol",
                "display_name":"GPT-5.6 Sol",
                "context_window":272000,
                "input_modalities":["text","image"],
                "supported_reasoning_levels":[{"effort":"high"}],
                "supports_search_tool":true
            },
            {"slug":"gpt-5.5","display_name":"GPT-5.5"},
            {"slug":"hidden-preview","visibility":"hide"}
        ]});
        let models = official_models_from_catalog(&catalog).unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].context_window, Some(272_000));
        assert_eq!(models[0].supports_image, Some(true));
        assert_eq!(models[0].supports_thinking, Some(true));
        assert_eq!(models[0].supports_web_search, Some(true));
        assert_eq!(
            filter_selected_models(models.clone(), None)
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-5.6-sol", "gpt-5.5"]
        );
        assert_eq!(
            filter_selected_models(models, Some(&["gpt-5.5".to_owned()]))
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-5.5"]
        );
    }

    #[test]
    fn explicit_empty_selection_removes_official_catalog_models() {
        let mut catalog = json!({"models":[{"slug":"gpt-5.5"}]});

        filter_official_catalog(&mut catalog, Some(&[])).unwrap();

        assert!(catalog["models"].as_array().unwrap().is_empty());
    }
}
