use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FusionModelProvider {
    Official,
    Provider,
}

pub(super) fn resolve_fusion_model(reference: &str) -> (FusionModelProvider, String) {
    if let Some(model) = reference.strip_prefix(OFFICIAL_MODEL_PREFIX) {
        return (FusionModelProvider::Official, model.to_owned());
    }
    (FusionModelProvider::Provider, reference.to_owned())
}

/// Fusion runs on every new user turn of a session: the panel advises, the
/// judge synthesizes, and the final model answers. Tool continuations within
/// that turn go straight to the final model, which owns the edits.
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
