use super::*;
use crate::config::ProviderPreset;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelRoute {
    Official,
    Direct,
    Fusion { profile_id: String },
}

pub fn model_route(model: &str) -> ModelRoute {
    if let Some(profile_id) = model.strip_prefix(FUSION_MODEL_PREFIX) {
        return ModelRoute::Fusion {
            profile_id: profile_id.to_owned(),
        };
    }
    if model
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("gpt-"))
        && !is_upstream_model_alias(model)
    {
        ModelRoute::Official
    } else {
        ModelRoute::Direct
    }
}

pub fn is_upstream_model_alias(model: &str) -> bool {
    canonical_upstream_model_alias(model) != model
}

pub fn canonical_upstream_model_alias(model: &str) -> &str {
    ProviderPreset::strip_model_provider_suffix(model)
        .filter(|canonical| canonical.to_ascii_lowercase().starts_with("gpt-"))
        .unwrap_or(model)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FusionModelProvider {
    Official,
    Upstream,
}

pub(super) fn resolve_fusion_model(
    reference: &str,
    upstream_provider: &str,
) -> (FusionModelProvider, String) {
    if let Some(model) = reference.strip_prefix(OFFICIAL_MODEL_PREFIX) {
        return (FusionModelProvider::Official, model.to_owned());
    }
    for prefix in ["upstream", upstream_provider] {
        if let Some(model) = reference
            .strip_prefix(prefix)
            .and_then(|value| value.strip_prefix(':'))
        {
            return (FusionModelProvider::Upstream, model.to_owned());
        }
    }
    (FusionModelProvider::Upstream, reference.to_owned())
}

pub fn should_fuse_turn(body: &Value) -> bool {
    if let Some(input) = body.get("input").and_then(Value::as_str) {
        return !input.trim().is_empty();
    }
    for item in input_items(body).rev() {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call_output" | "custom_tool_call_output" | "tool_search_output") => {
                return false;
            }
            Some("message") => match item.get("role").and_then(Value::as_str) {
                Some("user") => return true,
                Some("assistant") => return false,
                _ => {}
            },
            Some("function_call" | "custom_tool_call" | "tool_search_call") => return false,
            _ => {}
        }
    }
    false
}

pub(super) fn input_items(body: &Value) -> impl DoubleEndedIterator<Item = &Value> {
    body.get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

pub(super) fn is_user_message(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("message")
        && item.get("role").and_then(Value::as_str) == Some("user")
}
