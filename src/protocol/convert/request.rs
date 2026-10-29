use super::content::*;
use super::thinking::*;
use super::tool_map::ToolNameMap;
use super::tools::*;
use super::*;
use crate::protocol::compaction::SecretSource;
use crate::protocol::model_reasoning::{AnthropicThinkingKind, anthropic_thinking_kind};

#[derive(Clone, Debug)]
pub struct ConvertedRequest {
    pub request: MessageRequest,
    pub tool_names: ToolNameMap,
    /// The client omitted `max_output_tokens`, so `request.max_tokens` is the
    /// gateway's default and may be lowered to the provider's limit.
    pub max_tokens_defaulted: bool,
}

pub(crate) fn native_messages_body(
    body: &Value,
    request: &MessageRequest,
    original: &Value,
    compaction_secret: SecretSource<'_>,
) -> Result<Value, GatewayError> {
    let mut result = original.clone();
    let converted = serde_json::to_value(request)?;
    let object = result.as_object_mut().ok_or_else(|| {
        GatewayError::BadRequest("native Messages body must be an object".to_owned())
    })?;
    if let Some(fields) = converted.as_object() {
        object.extend(fields.clone());
    }
    let mut system = match original.get("system") {
        Some(Value::Array(blocks)) => blocks.clone(),
        Some(Value::String(text)) => vec![json!({"type":"text","text":text})],
        _ => Vec::new(),
    };
    let original_instructions = match original.get("system") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    };
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let guidance = instructions
        .strip_prefix(&original_instructions)
        .unwrap_or(instructions)
        .trim_start_matches('\n');
    if !guidance.is_empty() {
        system.push(json!({"type":"text","text":guidance}));
    }
    let mut messages: Vec<Value> = Vec::new();
    let mut lifted = Vec::new();
    for item in body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut converted = Vec::new();
        append_input_item(
            item,
            &mut lifted,
            &mut converted,
            false,
            body.get("model").and_then(Value::as_str),
            compaction_secret,
        )?;
        for message in converted {
            let mut value = serde_json::to_value(message)?;
            // Only ingress-created items carry this sidecar. Compression
            // summaries are new items and cannot inherit archived metadata.
            if let Some(raw) = item.get("__mixin_native_block")
                && let Some(block) = value
                    .get_mut("content")
                    .and_then(Value::as_array_mut)
                    .and_then(|blocks| (blocks.len() == 1).then(|| &mut blocks[0]))
            {
                restore_native_block_fields(block, raw);
            }
            if let Some(last) = messages.last_mut()
                && last.get("role") == value.get("role")
            {
                if let (Some(target), Some(source)) = (
                    last.get_mut("content").and_then(Value::as_array_mut),
                    value.get_mut("content").and_then(Value::as_array_mut),
                ) {
                    target.append(source);
                }
            } else {
                messages.push(value);
            }
        }
    }
    for message in &mut messages {
        if message.get("role").and_then(Value::as_str) == Some("assistant")
            && let Some(content) = message.get_mut("content").and_then(Value::as_array_mut)
        {
            // Match typed replay normalization after restoring native fields,
            // keeping signatures, cache controls and other sidecar data intact.
            content.sort_by_key(|block| {
                !matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("thinking" | "redacted_thinking")
                )
            });
        }
    }
    for block in lifted {
        system.push(serde_json::to_value(block)?);
    }
    if system.is_empty() {
        result.as_object_mut().map(|object| object.remove("system"));
    } else {
        result["system"] = Value::Array(system);
    }
    result["messages"] = Value::Array(messages);
    Ok(result)
}

fn restore_native_block_fields(converted: &mut Value, original: &Value) {
    match (converted, original) {
        (Value::Object(converted), Value::Object(original)) => {
            for (key, value) in original {
                if key == "name" {
                    converted.insert(key.clone(), value.clone());
                } else if let Some(existing) = converted.get_mut(key) {
                    restore_native_block_fields(existing, value);
                } else {
                    converted.insert(key.clone(), value.clone());
                }
            }
        }
        (Value::Array(converted), Value::Array(original)) if converted.len() == original.len() => {
            for (converted, original) in converted.iter_mut().zip(original) {
                restore_native_block_fields(converted, original);
            }
        }
        _ => {}
    }
}

/// Converts a streaming Responses request with hosted web search disabled.
pub fn responses_to_anthropic(
    body: &Value,
    thinking_mode: ThinkingMode,
    default_max_tokens: u64,
    compaction_secret: SecretSource<'_>,
) -> Result<ConvertedRequest, GatewayError> {
    if body.get("stream").and_then(Value::as_bool) != Some(true) {
        return Err(GatewayError::BadRequest(
            "Codex gateway currently requires stream=true".to_owned(),
        ));
    }
    let auto_thinking_kind = body
        .get("model")
        .and_then(Value::as_str)
        .and_then(anthropic_thinking_kind);
    responses_to_anthropic_with_model_reasoning_and_thinking_kind(
        body,
        None,
        normalized_reasoning(body).as_ref(),
        thinking_mode,
        default_max_tokens,
        None,
        false,
        auto_thinking_kind,
        compaction_secret,
    )
}

// Each argument is an independent per-request conversion input.
#[allow(clippy::too_many_arguments)]
pub(crate) fn responses_to_anthropic_with_model_reasoning_and_thinking_kind(
    body: &Value,
    model_override: Option<&str>,
    reasoning: Option<&Value>,
    thinking_mode: ThinkingMode,
    default_max_tokens: u64,
    web_search: Option<WebSearchTool<'_>>,
    use_mcp_bridge_names: bool,
    auto_thinking_kind: Option<AnthropicThinkingKind>,
    compaction_secret: SecretSource<'_>,
) -> Result<ConvertedRequest, GatewayError> {
    let model = match model_override {
        Some(model) => model.to_owned(),
        None => body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
            .to_owned(),
    };
    let client_max_tokens = body.get("max_output_tokens").and_then(Value::as_u64);
    let max_tokens = client_max_tokens.unwrap_or(default_max_tokens);
    let mut system = Vec::new();
    if let Some(instructions) = body
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        system.push(ContentBlock::Text {
            text: instructions.to_owned(),
        });
    }

    let mut messages = Vec::new();
    match body.get("input") {
        Some(Value::String(text)) => messages.push(Message {
            role: "user".to_owned(),
            content: vec![ContentBlock::Text { text: text.clone() }],
        }),
        Some(Value::Array(items)) => {
            let replay_model = body.get("model").and_then(Value::as_str);
            for item in items {
                append_input_item(
                    item,
                    &mut system,
                    &mut messages,
                    use_mcp_bridge_names,
                    replay_model,
                    compaction_secret,
                )?;
            }
        }
        Some(_) => {
            return Err(GatewayError::BadRequest(
                "input must be a string or array".to_owned(),
            ));
        }
        None => return Err(GatewayError::BadRequest("missing input".to_owned())),
    }
    if messages.is_empty() {
        return Err(GatewayError::BadRequest(
            "request has no Anthropic-compatible messages".to_owned(),
        ));
    }
    merge_consecutive_messages(&mut messages);

    let active_tools = collect_active_tools(body)?;
    let (tools, tool_names) = convert_tools(Some(&active_tools), web_search, use_mcp_bridge_names)?;
    let thinking = convert_thinking(max_tokens, reasoning, thinking_mode, auto_thinking_kind)?;
    let output_config = merge_anthropic_output_format(
        thinking.output_config,
        body.get("text").and_then(|text| text.get("format")),
    )?;
    let speed = body
        .get("service_tier")
        .and_then(Value::as_str)
        .filter(|tier| matches!(*tier, "fast" | "priority"))
        .map(|_| "fast".to_owned());
    let tool_choice = if tools.is_empty() {
        None
    } else {
        convert_tool_choice(
            body.get("tool_choice"),
            body.get("parallel_tool_calls").and_then(Value::as_bool),
        )
    };
    Ok(ConvertedRequest {
        request: MessageRequest {
            model,
            max_tokens,
            temperature: body.get("temperature").and_then(Value::as_f64),
            top_p: body.get("top_p").and_then(Value::as_f64),
            top_k: body.get("top_k").and_then(Value::as_u64),
            stream: true,
            speed,
            messages,
            system: if system.is_empty() {
                None
            } else {
                Some(system)
            },
            tools,
            tool_choice,
            thinking: thinking.thinking,
            output_config,
            metadata: None,
        },
        tool_names,
        max_tokens_defaulted: client_max_tokens.is_none(),
    })
}

pub(super) fn normalized_reasoning(body: &Value) -> Option<Value> {
    let mut reasoning = body.get("reasoning")?.clone();
    if reasoning.get("effort").and_then(Value::as_str) == Some("ultra") {
        reasoning["effort"] = Value::String("max".to_owned());
    }
    Some(reasoning)
}

pub(super) fn merge_anthropic_output_format(
    output_config: Option<Value>,
    format: Option<&Value>,
) -> Result<Option<Value>, GatewayError> {
    let Some(format) = format else {
        return Ok(output_config);
    };
    match format.get("type").and_then(Value::as_str) {
        None | Some("text") => Ok(output_config),
        Some("json_schema") => {
            let schema = format.get("schema").ok_or_else(|| {
                GatewayError::BadRequest("text.format json_schema missing schema".to_owned())
            })?;
            let mut output_config = output_config.unwrap_or_else(|| json!({}));
            output_config["format"] = json!({"type":"json_schema","schema":schema});
            Ok(Some(output_config))
        }
        Some(other) => Err(GatewayError::BadRequest(format!(
            "unsupported text format for Anthropic upstream: {other}"
        ))),
    }
}
