use serde_json::{Value, json};

const MAX_EMBEDDED_TOOL_IMAGE_BYTES: usize = 2 * 1024 * 1024;
const OMITTED_IMAGE_TEXT: &str = "[older tool image omitted by gateway]";

pub(super) fn compact_embedded_tool_images(body: &mut Value) -> usize {
    compact_embedded_tool_images_with_limit(body, MAX_EMBEDDED_TOOL_IMAGE_BYTES)
}

fn compact_embedded_tool_images_with_limit(body: &mut Value, max_bytes: usize) -> usize {
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return 0;
    };
    let embedded_image_bytes = input
        .iter()
        .filter_map(tool_output_parts)
        .flatten()
        .filter_map(embedded_image_url)
        .map(str::len)
        .sum::<usize>();
    if embedded_image_bytes <= max_bytes {
        return 0;
    }
    let Some(latest_image_output) = input
        .iter()
        .rposition(|item| tool_output_parts(item).is_some_and(has_embedded_image))
    else {
        return 0;
    };

    let mut removed = 0;
    for item in &mut input[..latest_image_output] {
        let Some(parts) = tool_output_parts_mut(item) else {
            continue;
        };
        let before = parts.len();
        parts.retain(|part| embedded_image_url(part).is_none());
        let removed_from_output = before - parts.len();
        removed += removed_from_output;
        if removed_from_output > 0 && parts.is_empty() {
            parts.push(json!({"type":"input_text","text":OMITTED_IMAGE_TEXT}));
        }
    }
    removed
}

fn tool_output_parts(item: &Value) -> Option<&[Value]> {
    if !is_tool_output(item) {
        return None;
    }
    item.get("output")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

fn tool_output_parts_mut(item: &mut Value) -> Option<&mut Vec<Value>> {
    if !is_tool_output(item) {
        return None;
    }
    item.get_mut("output").and_then(Value::as_array_mut)
}

fn is_tool_output(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call_output" | "custom_tool_call_output")
    )
}

fn has_embedded_image(parts: &[Value]) -> bool {
    parts.iter().any(|part| embedded_image_url(part).is_some())
}

fn embedded_image_url(part: &Value) -> Option<&str> {
    if part.get("type").and_then(Value::as_str) != Some("input_image") {
        return None;
    }
    part.get("image_url")
        .and_then(Value::as_str)
        .filter(|url| url.starts_with("data:image/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_tool_images_unchanged_when_they_fit_the_limit() {
        let mut body = json!({
            "input": [{
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [{
                    "type": "input_image",
                    "image_url": "data:image/png;base64,AAAA"
                }]
            }]
        });
        let original = body.clone();

        assert_eq!(compact_embedded_tool_images_with_limit(&mut body, 1024), 0);
        assert_eq!(body, original);
    }

    #[test]
    fn removes_only_embedded_images_from_tool_outputs_before_the_latest_one() {
        let mut body = json!({
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_image",
                        "image_url": "data:image/png;base64,user-image"
                    }]
                },
                {
                    "type": "function_call_output",
                    "call_id": "old_image_only",
                    "output": [{
                        "type": "input_image",
                        "image_url": "data:image/png;base64,old-image"
                    }]
                },
                {
                    "type": "custom_tool_call_output",
                    "call_id": "old_image_with_text",
                    "output": [
                        {"type":"input_text","text":"keep this observation"},
                        {
                            "type": "input_image",
                            "image_url": "data:image/png;base64,older-image"
                        },
                        {
                            "type": "input_image",
                            "image_url": "https://example.com/keep-remote.png"
                        }
                    ]
                },
                {
                    "type": "function_call_output",
                    "call_id": "latest_image",
                    "output": [{
                        "type": "input_image",
                        "image_url": "data:image/png;base64,latest-image"
                    }]
                }
            ]
        });

        assert_eq!(compact_embedded_tool_images_with_limit(&mut body, 1), 2);
        assert_eq!(
            body["input"][0]["content"][0]["image_url"],
            "data:image/png;base64,user-image"
        );
        assert_eq!(
            body["input"][1]["output"],
            json!([{"type":"input_text","text":OMITTED_IMAGE_TEXT}])
        );
        assert_eq!(
            body["input"][2]["output"],
            json!([
                {"type":"input_text","text":"keep this observation"},
                {
                    "type": "input_image",
                    "image_url": "https://example.com/keep-remote.png"
                }
            ])
        );
        assert_eq!(
            body["input"][3]["output"][0]["image_url"],
            "data:image/png;base64,latest-image"
        );
    }
}
