use super::content::*;
use super::thinking::*;
use super::tool_map::ToolNameMap;
use super::tools::*;
use super::*;
use crate::protocol::model_reasoning::{AnthropicThinkingKind, anthropic_thinking_kind};

#[derive(Clone, Debug)]
pub struct ConvertedRequest {
    pub request: MessageRequest,
    pub tool_names: ToolNameMap,
}

pub fn responses_to_anthropic(
    body: &Value,
    config: &GatewayConfig,
) -> Result<ConvertedRequest, GatewayError> {
    if body.get("stream").and_then(Value::as_bool) != Some(true) {
        return Err(GatewayError::BadRequest(
            "Codex gateway currently requires stream=true".to_owned(),
        ));
    }
    responses_to_anthropic_with_web_search(body, config, config.enable_web_search_tool, false)
}

pub(crate) fn responses_to_anthropic_with_web_search(
    body: &Value,
    config: &GatewayConfig,
    web_search_enabled: bool,
    use_mcp_bridge_names: bool,
) -> Result<ConvertedRequest, GatewayError> {
    let auto_thinking_kind = body
        .get("model")
        .and_then(Value::as_str)
        .and_then(anthropic_thinking_kind);
    responses_to_anthropic_with_web_search_and_thinking_kind(
        body,
        config,
        web_search_enabled,
        use_mcp_bridge_names,
        auto_thinking_kind,
    )
}

pub(crate) fn responses_to_anthropic_with_web_search_and_thinking_kind(
    body: &Value,
    config: &GatewayConfig,
    web_search_enabled: bool,
    use_mcp_bridge_names: bool,
    auto_thinking_kind: Option<AnthropicThinkingKind>,
) -> Result<ConvertedRequest, GatewayError> {
    responses_to_anthropic_with_model_and_thinking_kind(
        body,
        None,
        config,
        web_search_enabled,
        use_mcp_bridge_names,
        auto_thinking_kind,
    )
}

fn responses_to_anthropic_with_model_and_thinking_kind(
    body: &Value,
    model_override: Option<&str>,
    config: &GatewayConfig,
    web_search_enabled: bool,
    use_mcp_bridge_names: bool,
    auto_thinking_kind: Option<AnthropicThinkingKind>,
) -> Result<ConvertedRequest, GatewayError> {
    let reasoning = normalized_reasoning(body);
    responses_to_anthropic_with_model_reasoning_and_thinking_kind(
        body,
        model_override,
        reasoning.as_ref(),
        config,
        web_search_enabled,
        use_mcp_bridge_names,
        auto_thinking_kind,
    )
}

pub(crate) fn responses_to_anthropic_with_model_reasoning_and_thinking_kind(
    body: &Value,
    model_override: Option<&str>,
    reasoning: Option<&Value>,
    config: &GatewayConfig,
    web_search_enabled: bool,
    use_mcp_bridge_names: bool,
    auto_thinking_kind: Option<AnthropicThinkingKind>,
) -> Result<ConvertedRequest, GatewayError> {
    let model = match model_override {
        Some(model) => model.to_owned(),
        None => body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
            .to_owned(),
    };
    let max_tokens = body
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(config.default_max_tokens);
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
    let (tools, tool_names) = convert_tools(
        Some(&active_tools),
        config,
        use_mcp_bridge_names,
        web_search_enabled,
    )?;
    let thinking = convert_thinking(max_tokens, reasoning, config, auto_thinking_kind)?;
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
    })
}

fn normalized_reasoning(body: &Value) -> Option<Value> {
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
