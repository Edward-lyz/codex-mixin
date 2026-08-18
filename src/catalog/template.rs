use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::UPSTREAM_MODEL_MARKER;
use super::managed::is_managed_custom_model;

pub fn apply_web_search_capabilities(
    catalog: &mut Value,
    supported_models: &HashSet<String>,
) -> anyhow::Result<bool> {
    let supported_models = supported_models
        .iter()
        .map(|model| model.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let models = catalog
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("Codex model catalog has no models array"))?;
    let mut changed = false;
    for model in models
        .iter_mut()
        .filter(|model| is_managed_custom_model(model))
    {
        let slug = model
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("custom model is missing slug"))?;
        let upstream_model = model
            .get(UPSTREAM_MODEL_MARKER)
            .and_then(Value::as_str)
            .unwrap_or_else(|| slug.strip_suffix("-custom").unwrap_or(slug));
        let supported = supported_models.contains(&upstream_model.to_ascii_lowercase());
        if supported && model.get("web_search_tool_type").and_then(Value::as_str) != Some("text") {
            model["web_search_tool_type"] = json!("text");
            changed = true;
        } else if !supported && model.get("web_search_tool_type").is_some() {
            let model = model
                .as_object_mut()
                .expect("Codex catalog model must be an object");
            model.remove("web_search_tool_type");
            changed = true;
        }
        if model.get("use_responses_lite").and_then(Value::as_bool) != Some(false) {
            model["use_responses_lite"] = json!(false);
            changed = true;
        }
    }
    Ok(changed)
}

pub fn load_template_catalog(path: Option<&Path>) -> anyhow::Result<Option<Value>> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => {
            let codex_home = std::env::var("CODEX_HOME").ok().map_or_else(
                || {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
                    PathBuf::from(home).join(".codex")
                },
                PathBuf::from,
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

pub(super) fn fallback_template(default_context_window: u64) -> Value {
    json!({
        "slug": "placeholder",
        "display_name": "placeholder",
        "description": "",
        "base_instructions": super::FALLBACK_BASE_INSTRUCTIONS,
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
            "instructions_template": super::FALLBACK_BASE_INSTRUCTIONS
        }
    })
}

pub(super) fn template_model_context_window(
    template_catalog: Option<&Value>,
    slug: &str,
) -> Option<u64> {
    template_catalog
        .and_then(|catalog| catalog.get("models"))
        .and_then(Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find(|model| model.get("slug").and_then(Value::as_str) == Some(slug))
        })
        .and_then(|model| {
            model
                .get("context_window")
                .and_then(Value::as_u64)
                .or_else(|| model.get("max_context_window").and_then(Value::as_u64))
        })
}
