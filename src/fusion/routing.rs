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
