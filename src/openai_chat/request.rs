use super::content::append_input_item;
use super::tools::convert_tools;
use super::*;

#[derive(Clone, Debug)]
pub struct ConvertedChatRequest {
    pub request: Value,
    pub tool_names: ToolNameMap,
}

pub fn responses_to_openai_chat(body: &Value) -> Result<ConvertedChatRequest, GatewayError> {
    if body.get("stream").and_then(Value::as_bool) != Some(true) {
        return Err(GatewayError::BadRequest(
            "Codex gateway currently requires stream=true".to_owned(),
        ));
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?;
    let mut messages = Vec::new();
    if let Some(instructions) = body
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        messages.push(json!({"role":"system","content":instructions}));
    }
    match body.get("input") {
        Some(Value::String(text)) => messages.push(json!({"role":"user","content":text})),
        Some(Value::Array(items)) => {
            for item in items {
                append_input_item(item, &mut messages)?;
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
            "request has no OpenAI-compatible messages".to_owned(),
        ));
    }
    let active_tools = collect_active_tools(body)?;
    let (tools, tool_names) = convert_tools(Some(&active_tools))?;
    let mut request = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true}
    });
    if let Some(max_tokens) = body.get("max_output_tokens").and_then(Value::as_u64) {
        request["max_tokens"] = json!(max_tokens);
    }
    if let Some(temperature) = body.get("temperature") {
        request["temperature"] = temperature.clone();
    }
    if let Some(top_p) = body.get("top_p") {
        request["top_p"] = top_p.clone();
    }
    if let Some(format) = body.get("text").and_then(|text| text.get("format")) {
        match format.get("type").and_then(Value::as_str) {
            None | Some("text") => {}
            Some("json_schema") => {
                let name = format
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("structured_output");
                let schema = format.get("schema").ok_or_else(|| {
                    GatewayError::BadRequest("text.format json_schema missing schema".to_owned())
                })?;
                request["response_format"] = json!({
                    "type":"json_schema",
                    "json_schema":{
                        "name":name,
                        "strict":format.get("strict").and_then(Value::as_bool).unwrap_or(true),
                        "schema":schema
                    }
                });
            }
            Some(other) => {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported text format for OpenAI chat upstream: {other}"
                )));
            }
        }
    }
    if let Some(service_tier) = body.get("service_tier").and_then(Value::as_str) {
        request["service_tier"] = json!(if service_tier == "fast" {
            "priority"
        } else {
            service_tier
        });
    }
    if !tools.is_empty() {
        if let Some(tool_choice) = body.get("tool_choice") {
            request["tool_choice"] = tool_choice.clone();
        }
        if let Some(parallel_tool_calls) = body.get("parallel_tool_calls").and_then(Value::as_bool)
        {
            request["parallel_tool_calls"] = json!(parallel_tool_calls);
        }
        request["tools"] = Value::Array(tools);
    }
    Ok(ConvertedChatRequest {
        request,
        tool_names,
    })
}
