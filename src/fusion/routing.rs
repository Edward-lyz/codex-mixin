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
    if current_collaboration_mode(body) != Some(CollaborationMode::Plan) {
        return false;
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CollaborationMode {
    Plan,
    Other,
}

fn current_collaboration_mode(body: &Value) -> Option<CollaborationMode> {
    body.get("instructions")
        .and_then(Value::as_str)
        .and_then(collaboration_mode_from_text)
        .or_else(|| {
            input_items(body)
                .rev()
                .filter(|item| {
                    item.get("type").and_then(Value::as_str) == Some("message")
                        && matches!(
                            item.get("role").and_then(Value::as_str),
                            Some("developer" | "system")
                        )
                })
                .find_map(collaboration_mode_from_message)
        })
}

fn collaboration_mode_from_message(message: &Value) -> Option<CollaborationMode> {
    match message.get("content") {
        Some(Value::String(content)) => collaboration_mode_from_text(content),
        Some(Value::Array(parts)) => parts.iter().rev().find_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .and_then(collaboration_mode_from_text)
        }),
        _ => None,
    }
}

fn collaboration_mode_from_text(text: &str) -> Option<CollaborationMode> {
    const OPEN_TAG: &str = "<collaboration_mode>";
    const CLOSE_TAG: &str = "</collaboration_mode>";

    text.rmatch_indices(OPEN_TAG).find_map(|(start, _)| {
        let content = &text[start + OPEN_TAG.len()..];
        let end = content.find(CLOSE_TAG)?;
        let heading = content[..end]
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())?
            .trim_start_matches('#')
            .trim();
        let mode = heading
            .strip_prefix("Collaboration Mode:")
            .unwrap_or(heading)
            .trim();
        Some(if mode.eq_ignore_ascii_case("Plan") {
            CollaborationMode::Plan
        } else {
            CollaborationMode::Other
        })
    })
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
