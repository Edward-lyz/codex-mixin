use super::content::*;
use super::thinking::*;
use super::tool_map::ToolNameMap;
use super::tools::*;
use super::*;

#[derive(Clone, Debug)]
pub struct ConvertedRequest {
    pub request: MessageRequest,
    pub tool_names: ToolNameMap,
}

pub fn responses_to_anthropic(
    body: &Value,
    config: &GatewayConfig,
) -> Result<ConvertedRequest, GatewayError> {
    responses_to_anthropic_with_web_search(body, config, config.enable_web_search_tool)
}

pub(crate) fn responses_to_anthropic_with_web_search(
    body: &Value,
    config: &GatewayConfig,
    web_search_enabled: bool,
) -> Result<ConvertedRequest, GatewayError> {
    if body.get("stream").and_then(Value::as_bool) != Some(true) {
        return Err(GatewayError::BadRequest(
            "Codex gateway currently requires stream=true".to_owned(),
        ));
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let use_mcp_bridge_names = needs_baidu_fable_mcp_bridge(config.provider_preset, &model);
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
            for item in items {
                append_input_item(item, &mut system, &mut messages, use_mcp_bridge_names)?;
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
    let thinking = convert_thinking(&model, max_tokens, body.get("reasoning"), config)?;
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

/// Baidu OneAPI's Fable compatibility layer expects client tools under `mcp__codex__`.
fn needs_baidu_fable_mcp_bridge(provider: ProviderPreset, model: &str) -> bool {
    provider == ProviderPreset::BaiduOneApi && model.to_ascii_lowercase().contains("fable")
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
