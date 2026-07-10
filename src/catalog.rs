use std::collections::HashSet;

use serde_json::{Value, json};

use crate::anthropic::ModelInfo;
use crate::model_metadata::ModelMetadataResolver;

const FALLBACK_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. Work in the user's workspace, use tools carefully, and keep responses concise.";

pub fn codex_catalog_from_models(
    models: &[ModelInfo],
    default_context_window: u64,
    template_catalog: Option<&Value>,
) -> Value {
    codex_catalog_from_models_with_options(
        models,
        default_context_window,
        template_catalog,
        false,
        None,
    )
}

pub fn codex_oauth_proxy_catalog_from_models(
    models: &[ModelInfo],
    default_context_window: u64,
    template_catalog: Option<&Value>,
) -> Value {
    codex_catalog_from_models_with_options(
        models,
        default_context_window,
        template_catalog,
        true,
        None,
    )
}

pub fn codex_catalog_from_models_with_metadata(
    models: &[ModelInfo],
    default_context_window: u64,
    template_catalog: Option<&Value>,
    metadata: &ModelMetadataResolver,
) -> Value {
    codex_catalog_from_models_with_options(
        models,
        default_context_window,
        template_catalog,
        false,
        Some(metadata),
    )
}

pub fn codex_oauth_proxy_catalog_from_models_with_metadata(
    models: &[ModelInfo],
    default_context_window: u64,
    template_catalog: Option<&Value>,
    metadata: &ModelMetadataResolver,
) -> Value {
    codex_catalog_from_models_with_options(
        models,
        default_context_window,
        template_catalog,
        true,
        Some(metadata),
    )
}

fn codex_catalog_from_models_with_options(
    models: &[ModelInfo],
    default_context_window: u64,
    template_catalog: Option<&Value>,
    include_template_models: bool,
    metadata: Option<&ModelMetadataResolver>,
) -> Value {
    let template = template_catalog
        .and_then(|catalog| catalog.get("models"))
        .and_then(Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find(|model| model.get("slug").and_then(Value::as_str) == Some("gpt-5.4-mini"))
                .or_else(|| models.first())
        });
    let mut generated = template_catalog
        .and_then(|catalog| catalog.get("models"))
        .and_then(Value::as_array)
        .filter(|_| include_template_models)
        .cloned()
        .unwrap_or_default();
    let mut custom_models = models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            let mut item = template
                .cloned()
                .unwrap_or_else(|| fallback_template(default_context_window));
            let is_gpt = is_gpt_model(&model.id);
            let slug = if include_template_models && is_gpt {
                format!("{}-custom", model.id)
            } else {
                model.id.clone()
            };
            let display_name = if include_template_models && is_gpt {
                format!("{} (Custom)", model.id)
            } else {
                model.id.clone()
            };
            item["slug"] = json!(slug);
            item["display_name"] = json!(display_name);
            let mut description = model
                .description
                .clone()
                .unwrap_or_else(|| "Custom upstream model exposed through codex-mixin".to_owned());
            for detail in [&model.ratio, &model.price_type]
                .into_iter()
                .filter_map(Option::as_deref)
                .filter(|value| !value.is_empty())
            {
                description.push_str(" | ");
                description.push_str(detail);
            }
            item["description"] = json!(description);
            item["multi_agent_version"] = json!("v2");
            if item.get("base_instructions").is_none() {
                item["base_instructions"] = json!(FALLBACK_BASE_INSTRUCTIONS);
            }
            let metadata = metadata
                .map(|resolver| resolver.resolve(&model.id, default_context_window))
                .unwrap_or_else(|| {
                    ModelMetadataResolver::empty().resolve(&model.id, default_context_window)
                });
            let context_window = model.context_window.unwrap_or(metadata.context_window);
            let input_modalities = match model.supports_image {
                Some(true) => vec!["text".to_owned(), "image".to_owned()],
                Some(false) => vec!["text".to_owned()],
                None => metadata.input_modalities,
            };
            item["context_window"] = json!(context_window);
            item["max_context_window"] = json!(context_window);
            item["input_modalities"] = json!(input_modalities);
            item["priority"] = json!(100 + index as u64);
            item["visibility"] = json!("list");
            item["supported_in_api"] = json!(true);
            let supports_search_tool = supports_anthropic_web_search(&model.id);
            item["supports_search_tool"] = json!(supports_search_tool);
            if supports_search_tool {
                item["web_search_tool_type"] = json!("text");
            } else if let Some(item) = item.as_object_mut() {
                item.remove("web_search_tool_type");
            }
            item
        })
        .collect::<Vec<_>>();
    generated.append(&mut custom_models);
    for model in &mut generated {
        ensure_instruction_fields(model);
    }
    json!({ "models": generated })
}

pub fn refresh_managed_oauth_catalog(
    official_catalog: &Value,
    managed_catalog: &Value,
) -> anyhow::Result<Value> {
    let mut refreshed = official_catalog
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("official Codex catalog must be an object"))?;
    let mut models = official_catalog
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("official Codex catalog has no models array"))?
        .clone();
    let managed_models = managed_catalog
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("managed Codex catalog has no models array"))?;
    let mut slugs = HashSet::with_capacity(models.len() + managed_models.len());
    for model in &models {
        let slug = model
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("official Codex model is missing slug"))?;
        if !slugs.insert(slug.to_owned()) {
            anyhow::bail!("duplicate model slug in official Codex catalog: {slug}");
        }
    }

    for model in managed_models.iter().filter(|model| {
        model
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|description| {
                description.starts_with("Custom upstream model exposed through codex-")
            })
            || model
                .get("slug")
                .and_then(Value::as_str)
                .is_some_and(|slug| slug.ends_with("-custom"))
    }) {
        let mut model = model.clone();
        let slug = model
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("custom model is missing slug"))?
            .to_owned();
        if !slugs.insert(slug.clone()) {
            anyhow::bail!("custom model slug collides with existing catalog: {slug}");
        }
        model["multi_agent_version"] = json!("v2");
        let supports_search_tool = model
            .get("supports_search_tool")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| supports_anthropic_web_search(&slug));
        model["supports_search_tool"] = json!(supports_search_tool);
        if supports_search_tool {
            model["web_search_tool_type"] = json!("text");
        } else if let Some(model) = model.as_object_mut() {
            model.remove("web_search_tool_type");
        }
        ensure_instruction_fields(&mut model);
        models.push(model);
    }
    for model in &mut models {
        ensure_instruction_fields(model);
    }

    refreshed.insert("models".to_owned(), Value::Array(models));
    Ok(Value::Object(refreshed))
}

fn ensure_instruction_fields(model: &mut Value) {
    if model.get("base_instructions").is_none() {
        model["base_instructions"] = json!(FALLBACK_BASE_INSTRUCTIONS);
    }
    if model
        .pointer("/model_messages/instructions_template")
        .is_none()
    {
        model["model_messages"] = json!({
            "instructions_template": FALLBACK_BASE_INSTRUCTIONS
        });
    }
}

fn is_gpt_model(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("gpt-")
}

fn supports_anthropic_web_search(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    ["claude", "sonnet", "opus", "haiku", "fable", "mythos"]
        .iter()
        .any(|needle| model.contains(needle))
}

pub fn load_template_catalog(path: Option<&std::path::Path>) -> anyhow::Result<Option<Value>> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => {
            let codex_home = std::env::var("CODEX_HOME").ok().map_or_else(
                || {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
                    std::path::PathBuf::from(home).join(".codex")
                },
                std::path::PathBuf::from,
            );
            codex_home.join("models_cache.json")
        }
    };
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    let parsed = serde_json::from_str(&raw)?;
    Ok(Some(parsed))
}

fn fallback_template(default_context_window: u64) -> Value {
    json!({
        "slug": "placeholder",
        "display_name": "placeholder",
        "description": "",
        "base_instructions": FALLBACK_BASE_INSTRUCTIONS,
        "experimental_supported_tools": [],
        "priority": 100,
        "shell_type": "shell_command",
        "support_verbosity": false,
        "supported_in_api": true,
        "supported_reasoning_levels": [
            {"effort":"low","description":"Fast responses with lighter reasoning"},
            {"effort":"medium","description":"Balanced reasoning"},
            {"effort":"high","description":"Greater reasoning depth"},
            {"effort":"xhigh","description":"Extra high reasoning depth"}
        ],
        "supports_parallel_tool_calls": true,
        "supports_reasoning_summaries": false,
        "truncation_policy": {"mode":"tokens","limit":10000},
        "visibility": "list",
        "context_window": default_context_window,
        "max_context_window": default_context_window,
        "input_modalities": ["text", "image"],
        "apply_patch_tool_type": "freeform",
        "model_messages": {
            "instructions_template": FALLBACK_BASE_INSTRUCTIONS
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_codex_catalog_from_live_model_shape() {
        let models = vec![ModelInfo {
            id: "DeepSeek-V4-Flash".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];
        let catalog = codex_catalog_from_models(&models, 1_000_000, None);
        assert_eq!(catalog["models"][0]["slug"], "DeepSeek-V4-Flash");
        assert_eq!(
            catalog["models"][0]["base_instructions"],
            FALLBACK_BASE_INSTRUCTIONS
        );
        assert_eq!(
            catalog["models"][0]["model_messages"]["instructions_template"],
            FALLBACK_BASE_INSTRUCTIONS
        );
        assert_eq!(catalog["models"][0]["context_window"], 1_000_000);
        assert_eq!(catalog["models"][0]["supports_search_tool"], false);
    }

    #[test]
    fn applies_model_metadata_to_context_window_and_modalities() {
        let metadata = ModelMetadataResolver::from_json(&json!({
            "fireworks_ai/minimax-m3": {
                "mode": "chat",
                "max_input_tokens": 512000,
                "max_output_tokens": 512000,
                "supports_vision": true
            }
        }))
        .unwrap();
        let models = vec![ModelInfo {
            id: "MiniMax-M3".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];
        let catalog = codex_catalog_from_models_with_metadata(&models, 1_000_000, None, &metadata);
        assert_eq!(catalog["models"][0]["context_window"], 512_000);
        assert_eq!(catalog["models"][0]["max_context_window"], 512_000);
        assert_eq!(
            catalog["models"][0]["input_modalities"],
            json!(["text", "image"])
        );
    }

    #[test]
    fn provider_metadata_overrides_catalog_description_and_capabilities() {
        let models = vec![ModelInfo {
            id: "DeepSeek-V4-Flash".to_owned(),
            description: Some("Fast coding model".to_owned()),
            ratio: Some("0.2x".to_owned()),
            price_type: Some("Value".to_owned()),
            context_window: Some(1_024_000),
            supports_image: Some(false),
            supports_thinking: Some(true),
            ..ModelInfo::default()
        }];

        let catalog = codex_catalog_from_models(&models, 1_000_000, None);

        assert_eq!(
            catalog["models"][0]["description"],
            "Fast coding model | 0.2x | Value"
        );
        assert_eq!(catalog["models"][0]["context_window"], 1_024_000);
        assert_eq!(catalog["models"][0]["input_modalities"], json!(["text"]));
    }

    #[test]
    fn marks_claude_family_models_as_search_capable() {
        let models = vec![ModelInfo {
            id: "Claude Sonnet 5".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];
        let catalog = codex_catalog_from_models(&models, 1_000_000, None);
        assert_eq!(catalog["models"][0]["supports_search_tool"], true);
        assert_eq!(catalog["models"][0]["web_search_tool_type"], "text");
    }

    #[test]
    fn removes_inherited_search_type_from_unsupported_models() {
        let template = json!({
            "models": [{
                "slug": "gpt-template",
                "display_name": "Template",
                "base_instructions": "test",
                "web_search_tool_type": "text_and_image"
            }]
        });
        let models = vec![ModelInfo {
            id: "DeepSeek-V4-Flash".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];

        let catalog = codex_catalog_from_models(&models, 1_000_000, Some(&template));

        assert_eq!(catalog["models"][0]["supports_search_tool"], false);
        assert!(catalog["models"][0].get("web_search_tool_type").is_none());
    }

    #[test]
    fn oauth_proxy_catalog_keeps_official_gpt_and_aliases_custom_gpt() {
        let template = json!({
            "models": [
                {"slug":"gpt-5.5","display_name":"GPT-5.5","context_window":272000}
            ]
        });
        let models = vec![ModelInfo {
            id: "gpt-5.5".to_owned(),
            object: Some("model".to_owned()),
            created: Some(1),
            owned_by: Some("custom".to_owned()),
            ..ModelInfo::default()
        }];
        let catalog = codex_oauth_proxy_catalog_from_models(&models, 1_000_000, Some(&template));
        assert_eq!(catalog["models"][0]["slug"], "gpt-5.5");
        assert_eq!(catalog["models"][1]["slug"], "gpt-5.5-custom");
        assert_eq!(catalog["models"][1]["display_name"], "gpt-5.5 (Custom)");
        assert_eq!(
            catalog["models"][1]["base_instructions"],
            FALLBACK_BASE_INSTRUCTIONS
        );
        assert_eq!(
            catalog["models"][1]["model_messages"]["instructions_template"],
            FALLBACK_BASE_INSTRUCTIONS
        );
        assert_eq!(catalog["models"][1]["multi_agent_version"], "v2");
    }

    #[test]
    fn refreshes_official_models_without_dropping_custom_models() {
        let current_official = json!({
            "client_version": "1.2.3",
            "etag": "catalog-etag",
            "models": [
                {"slug":"gpt-5.6-sol","display_name":"GPT-5.6-Sol"},
                {"slug":"gpt-5.5","display_name":"GPT-5.5"}
            ]
        });
        let managed = json!({
            "models": [
                {"slug":"gpt-5.5","display_name":"GPT-5.5"},
                {
                    "slug":"DeepSeek-V4-Flash",
                    "display_name":"DeepSeek-V4-Flash",
                    "description":"Custom upstream model exposed through codex-mixin",
                    "supports_search_tool":false,
                    "web_search_tool_type":"text_and_image"
                },
                {
                    "slug":"gpt-5.5-custom",
                    "display_name":"gpt-5.5 (Custom)",
                    "description":"Custom upstream model exposed through codex-mixin"
                }
            ]
        });

        let refreshed = refresh_managed_oauth_catalog(&current_official, &managed).unwrap();
        let slugs = refreshed["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            slugs,
            vec![
                "gpt-5.6-sol",
                "gpt-5.5",
                "DeepSeek-V4-Flash",
                "gpt-5.5-custom"
            ]
        );
        assert_eq!(refreshed["models"][2]["multi_agent_version"], "v2");
        assert_eq!(refreshed["models"][3]["multi_agent_version"], "v2");
        assert!(refreshed["models"][2].get("web_search_tool_type").is_none());
        for model in refreshed["models"].as_array().unwrap() {
            assert_eq!(model["base_instructions"], FALLBACK_BASE_INSTRUCTIONS);
            assert_eq!(
                model["model_messages"]["instructions_template"],
                FALLBACK_BASE_INSTRUCTIONS
            );
        }
        assert_eq!(refreshed["client_version"], "1.2.3");
        assert_eq!(refreshed["etag"], "catalog-etag");
    }

    #[test]
    fn rejects_custom_slug_collisions_during_refresh() {
        let official = json!({"models":[{"slug":"gpt-5.6-sol"}]});
        let managed = json!({"models":[{
            "slug":"gpt-5.6-sol",
            "description":"Custom upstream model exposed through codex-mixin"
        }]});
        let error = refresh_managed_oauth_catalog(&official, &managed).unwrap_err();
        assert!(error.to_string().contains("collides"));
    }
}
