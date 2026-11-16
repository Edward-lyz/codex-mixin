use serde_json::{Value, json};

fn is_message_tool(namespace: Option<&str>, name: &str) -> bool {
    namespace == Some("collaboration")
        && matches!(name, "spawn_agent" | "send_message" | "followup_task")
}

/// Codex otherwise treats these arguments as encrypted even when the upstream
/// protocol only supports plaintext. An explicit empty list selects its
/// DirectPlaintextMessage delivery path and produces input_text agent messages.
pub(crate) fn mark_plaintext_call(item: &mut Value) {
    if item.get("type").and_then(Value::as_str) == Some("function_call")
        && is_message_tool(
            item.get("namespace").and_then(Value::as_str),
            item.get("name").and_then(Value::as_str).unwrap_or(""),
        )
        && item
            .get("encrypted_function_args")
            .is_none_or(Value::is_null)
    {
        item["encrypted_function_args"] = json!([]);
    }
}

pub(crate) fn mark_plaintext_event(payload: &mut Value) {
    if let Some(item) = payload.get_mut("item") {
        mark_plaintext_call(item);
    }
    if let Some(output) = payload
        .get_mut("response")
        .and_then(|response| response.get_mut("output"))
        .and_then(Value::as_array_mut)
    {
        for item in output {
            mark_plaintext_call(item);
        }
    }
}

pub(crate) fn prepare_schema(schema: &mut Value, namespace: Option<&str>, name: &str) {
    if is_message_tool(namespace, name)
        && let Some(message) = schema
            .pointer_mut("/properties/message")
            .and_then(Value::as_object_mut)
    {
        message.remove("encrypted");
    }
}

/// Only schema annotations move out of the outbound request. Keep their paths
/// so the retained request and its native history can be restored without
/// cloning the conversation or guessing whether opaque payloads are plaintext.
pub(crate) struct RequestRestore(Vec<(String, Value)>);

pub(crate) fn prepare_request(body: &mut Value) -> RequestRestore {
    let mut saved = Vec::new();
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        prepare_tools(tools, "/tools", &mut saved);
    }
    if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
        for (index, item) in input.iter_mut().enumerate() {
            if matches!(
                item.get("type").and_then(Value::as_str),
                Some("additional_tools" | "tool_search_output")
            ) && let Some(tools) = item.get_mut("tools").and_then(Value::as_array_mut)
            {
                prepare_tools(tools, &format!("/input/{index}/tools"), &mut saved);
            }
        }
    }
    RequestRestore(saved)
}

fn prepare_tools(tools: &mut [Value], prefix: &str, saved: &mut Vec<(String, Value)>) {
    for (index, tool) in tools.iter_mut().enumerate() {
        if tool.get("type").and_then(Value::as_str) != Some("namespace")
            || tool.get("name").and_then(Value::as_str) != Some("collaboration")
        {
            continue;
        }
        let Some(nested) = tool.get_mut("tools").and_then(Value::as_array_mut) else {
            continue;
        };
        for (nested_index, function) in nested.iter_mut().enumerate() {
            if function.get("type").and_then(Value::as_str) != Some("function")
                || !is_message_tool(
                    Some("collaboration"),
                    function.get("name").and_then(Value::as_str).unwrap_or(""),
                )
            {
                continue;
            }
            for schema in ["parameters", "input_schema", "inputSchema"] {
                if let Some(message) = function
                    .get_mut(schema)
                    .and_then(|schema| schema.pointer_mut("/properties/message"))
                    .and_then(Value::as_object_mut)
                    && let Some(annotation) = message.remove("encrypted")
                {
                    saved.push((
                        format!(
                            "{prefix}/{index}/tools/{nested_index}/{schema}/properties/message"
                        ),
                        annotation,
                    ));
                }
            }
        }
    }
}

pub(crate) fn restore_request(body: &mut Value, saved: RequestRestore) {
    for (path, annotation) in saved.0 {
        if let Some(message) = body.pointer_mut(&path).and_then(Value::as_object_mut) {
            message.insert("encrypted".to_owned(), annotation);
        }
    }
}

#[cfg(test)]
mod tests;
