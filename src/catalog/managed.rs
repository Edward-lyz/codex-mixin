use std::collections::HashMap;

use serde_json::{Value, json};

use crate::protocol::model_reasoning::resolve_model_reasoning;

use super::{
    CUSTOM_MODEL_MARKER, FALLBACK_BASE_INSTRUCTIONS, SUPPORTS_THINKING_MARKER,
    UPSTREAM_MODEL_MARKER,
};

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
    let official_context_windows = models
        .iter()
        .filter_map(|model| {
            Some((
                model.get("slug")?.as_str()?.to_owned(),
                model_context_window(model)?,
            ))
        })
        .collect::<HashMap<_, _>>();
    let mut slugs = std::collections::HashSet::with_capacity(models.len() + managed_models.len());
    for model in &models {
        let slug = model
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("official Codex model is missing slug"))?;
        if !slugs.insert(slug.to_owned()) {
            anyhow::bail!("duplicate model slug in official Codex catalog: {slug}");
        }
    }

    for model in managed_models
        .iter()
        .filter(|model| is_managed_custom_model(model))
    {
        let mut model = model.clone();
        remove_official_lifecycle_metadata(&mut model);
        let slug = model
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("custom model is missing slug"))?
            .to_owned();
        if !slugs.insert(slug.clone()) {
            anyhow::bail!("custom model slug collides with existing catalog: {slug}");
        }
        let reasoning_model_id = model
            .get(UPSTREAM_MODEL_MARKER)
            .and_then(Value::as_str)
            .unwrap_or(&slug)
            .to_owned();
        let advertised_support = model.get(SUPPORTS_THINKING_MARKER).and_then(Value::as_bool);
        apply_model_reasoning_capabilities(&mut model, &reasoning_model_id, advertised_support);
        let supports_search_tool = model
            .get("supports_search_tool")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        model["supports_search_tool"] = json!(supports_search_tool);
        clamp_managed_gpt_context(&mut model, &official_context_windows);
        enable_fast_service_tier(&mut model);
        models.push(model);
    }
    for model in &mut models {
        ensure_instruction_fields(model);
    }

    refreshed.insert("models".to_owned(), Value::Array(models));
    Ok(Value::Object(refreshed))
}

pub fn migrate_managed_model_metadata(catalog: &mut Value) -> anyhow::Result<bool> {
    let models = catalog
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("managed Codex catalog has no models array"))?;
    let mut changed = false;
    for model in models
        .iter_mut()
        .filter(|model| is_managed_custom_model(model))
    {
        changed |= remove_official_lifecycle_metadata(model);
    }
    Ok(changed)
}

pub(super) fn remove_official_lifecycle_metadata(model: &mut Value) -> bool {
    let Some(model) = model.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for field in [
        "upgrade",
        "availability_nux",
        "retirement_at",
        "migration_markdown",
    ] {
        changed |= model.remove(field).is_some();
    }
    changed
}

pub(super) fn enable_fast_service_tier(model: &mut Value) {
    model["additional_speed_tiers"] = json!(["fast"]);
    model["service_tiers"] = json!([{
        "id": "priority",
        "name": "Fast",
        "description": "Requests faster processing when the upstream provider supports it"
    }]);
}

pub(super) fn apply_model_reasoning_capabilities(
    model: &mut Value,
    model_id: &str,
    advertised_support: Option<bool>,
) {
    let model = model
        .as_object_mut()
        .expect("Codex catalog model must be an object");
    let Some(capabilities) = resolve_model_reasoning(model_id, advertised_support) else {
        model.remove("default_reasoning_level");
        model.remove("supported_reasoning_levels");
        model.remove("multi_agent_version");
        return;
    };
    model.insert(
        "default_reasoning_level".to_owned(),
        json!(capabilities.default_effort),
    );
    model.insert(
        "supported_reasoning_levels".to_owned(),
        Value::Array(
            capabilities
                .supported_levels
                .iter()
                .map(|level| {
                    json!({
                        "effort": level.effort,
                        "description": level.description,
                    })
                })
                .collect(),
        ),
    );
    match capabilities.multi_agent_version {
        Some(version) => {
            model.insert("multi_agent_version".to_owned(), json!(version));
        }
        None => {
            model.remove("multi_agent_version");
        }
    }
}

pub(super) fn ensure_instruction_fields(model: &mut Value) {
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

fn model_context_window(model: &Value) -> Option<u64> {
    model
        .get("context_window")
        .and_then(Value::as_u64)
        .or_else(|| model.get("max_context_window").and_then(Value::as_u64))
}

fn clamp_managed_gpt_context(model: &mut Value, official_context_windows: &HashMap<String, u64>) {
    let Some(upstream_model) = model
        .get(UPSTREAM_MODEL_MARKER)
        .and_then(Value::as_str)
        .filter(|model| is_gpt_model(model))
    else {
        return;
    };
    let Some(official_context_window) = official_context_windows.get(upstream_model).copied()
    else {
        return;
    };
    // Preserve provider-advertised context windows. Only fill missing GPT windows from
    // the official catalog entry with the same bare upstream id.
    if model_context_window(model).is_some() {
        return;
    }
    model["context_window"] = json!(official_context_window);
    model["max_context_window"] = json!(official_context_window);
}

pub(super) fn is_managed_custom_model(model: &Value) -> bool {
    model.get(CUSTOM_MODEL_MARKER).and_then(Value::as_bool) == Some(true)
        || model
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|description| {
                description.starts_with("Custom upstream model exposed through codex-")
            })
}

fn is_gpt_model(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("gpt-")
}
