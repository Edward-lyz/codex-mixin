use serde_json::{Value, json};

use crate::anthropic::ModelInfo;
use crate::model_metadata::ModelMetadataResolver;

use super::managed::{apply_model_reasoning_capabilities, ensure_instruction_fields};
use super::template::{fallback_template, template_model_context_window};
use super::{
    CUSTOM_MODEL_MARKER, FALLBACK_BASE_INSTRUCTIONS, SUPPORTS_THINKING_MARKER,
    UPSTREAM_MODEL_MARKER,
};

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
        Some("custom"),
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
        None,
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
        Some("custom"),
    )
}

pub fn codex_oauth_proxy_catalog_from_models_with_metadata_for_provider(
    models: &[ModelInfo],
    default_context_window: u64,
    template_catalog: Option<&Value>,
    metadata: &ModelMetadataResolver,
    provider_suffix: &str,
) -> Value {
    codex_catalog_from_models_with_options(
        models,
        default_context_window,
        template_catalog,
        true,
        Some(metadata),
        Some(provider_suffix),
    )
}

pub fn codex_oauth_proxy_catalog_from_aggregated_models_with_metadata(
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
        None,
    )
}

fn codex_catalog_from_models_with_options(
    models: &[ModelInfo],
    default_context_window: u64,
    template_catalog: Option<&Value>,
    include_template_models: bool,
    metadata: Option<&ModelMetadataResolver>,
    provider_suffix: Option<&str>,
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
            let owned_provider = model
                .owned_by
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != "codex-mixin");
            let effective_provider_suffix = provider_suffix.or(owned_provider);
            // /v1/models may already return provider-qualified IDs. Strip that suffix so
            // catalog markers, metadata lookup, and GPT clamping use the bare upstream id.
            let upstream_model_id = bare_upstream_model_id(&model.id, effective_provider_suffix);
            let is_gpt = is_gpt_model(&upstream_model_id);
            let slug = if include_template_models
                && let Some(provider_suffix) = effective_provider_suffix
            {
                format!("{upstream_model_id}-{provider_suffix}")
            } else {
                model.id.clone()
            };
            let display_name = model.display_name.clone().unwrap_or_else(|| {
                if include_template_models && let Some(provider_suffix) = effective_provider_suffix
                {
                    let provider = if provider_suffix == "custom" {
                        "Custom"
                    } else {
                        provider_suffix
                    };
                    format!("{upstream_model_id} ({provider})")
                } else {
                    model.id.clone()
                }
            });
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
            item[CUSTOM_MODEL_MARKER] = json!(true);
            item[UPSTREAM_MODEL_MARKER] = json!(upstream_model_id);
            if let Some(supports_thinking) = model.supports_thinking {
                item[SUPPORTS_THINKING_MARKER] = json!(supports_thinking);
            } else if let Some(item) = item.as_object_mut() {
                item.remove(SUPPORTS_THINKING_MARKER);
            }
            if item.get("base_instructions").is_none() {
                item["base_instructions"] = json!(FALLBACK_BASE_INSTRUCTIONS);
            }
            let metadata = metadata
                .map(|resolver| resolver.resolve(&upstream_model_id, default_context_window))
                .unwrap_or_else(|| {
                    ModelMetadataResolver::empty()
                        .resolve(&upstream_model_id, default_context_window)
                });
            let mut context_window = model.context_window.unwrap_or(metadata.context_window);
            // Only fall back to official GPT windows when the provider did not advertise one.
            // Provider-specific SSOT (for example OpenCode Go via models.dev) may intentionally
            // expose a larger context than the official Codex GPT entry.
            if model.context_window.is_none()
                && include_template_models
                && is_gpt
                && let Some(official_context_window) =
                    template_model_context_window(template_catalog, &upstream_model_id)
            {
                context_window = context_window.min(official_context_window);
            }
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
            item["supports_search_tool"] = json!(model.supports_tool_search.unwrap_or(false));
            item["use_responses_lite"] = json!(false);
            super::managed::enable_fast_service_tier(&mut item);
            apply_model_reasoning_capabilities(
                &mut item,
                &upstream_model_id,
                model.supports_thinking,
            );
            if model.supports_web_search == Some(true) {
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

fn is_gpt_model(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("gpt-")
}

fn bare_upstream_model_id(model_id: &str, provider_suffix: Option<&str>) -> String {
    let Some(provider_suffix) = provider_suffix else {
        return model_id.to_owned();
    };
    let suffix = format!("-{provider_suffix}");
    match model_id.strip_suffix(&suffix) {
        Some(bare) if !bare.is_empty() => bare.to_owned(),
        _ => model_id.to_owned(),
    }
}
