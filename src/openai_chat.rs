use serde_json::{Value, json};

use crate::convert::{ToolNameMap, sanitize_tool_name};
use crate::error::GatewayError;

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
    let (tools, tool_names) = convert_tools(body.get("tools"))?;
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
    if !tools.is_empty() {
        request["tools"] = Value::Array(tools);
    }
    if let Some(tool_choice) = body.get("tool_choice") {
        request["tool_choice"] = tool_choice.clone();
    }
    Ok(ConvertedChatRequest {
        request,
        tool_names,
    })
}

fn append_input_item(item: &Value, messages: &mut Vec<Value>) -> Result<(), GatewayError> {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message");
    match item_type {
        "message" => {
            let role = item
                .get("role")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::BadRequest("message item missing role".to_owned()))?;
            let role = match role {
                "developer" | "system" => "system",
                "user" | "assistant" => role,
                other => {
                    return Err(GatewayError::BadRequest(format!(
                        "unsupported message role: {other}"
                    )));
                }
            };
            messages.push(json!({"role":role,"content":convert_content(item.get("content"))?}));
        }
        "function_call" => {
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("function_call missing call_id".to_owned())
            })?;
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::BadRequest("function_call missing name".to_owned()))?;
            let arguments = match item.get("arguments") {
                Some(Value::String(arguments)) => arguments.clone(),
                Some(Value::Object(_)) => serde_json::to_string(&item["arguments"])?,
                Some(Value::Null) | None => "{}".to_owned(),
                Some(other) => {
                    return Err(GatewayError::BadRequest(format!(
                        "function_call arguments must be a JSON string or object, got {other}"
                    )));
                }
            };
            messages.push(json!({
                "role":"assistant",
                "content": null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {"name": sanitize_tool_name(name), "arguments": arguments}
                }]
            }));
        }
        "function_call_output" => {
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("function_call_output missing call_id".to_owned())
            })?;
            let output = item.get("output").and_then(Value::as_str).unwrap_or("");
            messages.push(json!({"role":"tool","tool_call_id":call_id,"content":output}));
        }
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unsupported input item type: {other}"
            )));
        }
    }
    Ok(())
}

fn convert_content(content: Option<&Value>) -> Result<Value, GatewayError> {
    match content {
        Some(Value::String(text)) => Ok(json!(text)),
        Some(Value::Array(parts)) => {
            let mut converted = Vec::new();
            for part in parts {
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("text");
                match part_type {
                    "input_text" | "output_text" | "text" => {
                        let text = part.get("text").and_then(Value::as_str).ok_or_else(|| {
                            GatewayError::BadRequest(format!("{part_type} missing text"))
                        })?;
                        converted.push(json!({"type":"text","text":text}));
                    }
                    "input_image" => {
                        let image_url =
                            part.get("image_url")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    GatewayError::BadRequest(
                                        "input_image missing image_url".to_owned(),
                                    )
                                })?;
                        converted.push(json!({"type":"image_url","image_url":{"url":image_url}}));
                    }
                    other => {
                        return Err(GatewayError::BadRequest(format!(
                            "unsupported content part type: {other}"
                        )));
                    }
                }
            }
            Ok(Value::Array(converted))
        }
        Some(_) => Err(GatewayError::BadRequest(
            "message content must be a string or array".to_owned(),
        )),
        None => Ok(json!("")),
    }
}

fn convert_tools(tools: Option<&Value>) -> Result<(Vec<Value>, ToolNameMap), GatewayError> {
    let Some(Value::Array(tools)) = tools else {
        return Ok((Vec::new(), ToolNameMap::default()));
    };
    let mut result = Vec::new();
    let mut tool_names = ToolNameMap::default();
    for tool in tools {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => {
                let (converted, openai_name) = convert_function_tool(tool, None)?;
                let sanitized = converted["function"]["name"]
                    .as_str()
                    .expect("converted function tool missing name")
                    .to_owned();
                tool_names.insert(sanitized, openai_name);
                result.push(converted);
            }
            Some("namespace") => {
                let namespace = tool.get("name").and_then(Value::as_str).ok_or_else(|| {
                    GatewayError::BadRequest("namespace tool missing name".to_owned())
                })?;
                let nested_tools =
                    tool.get("tools").and_then(Value::as_array).ok_or_else(|| {
                        GatewayError::BadRequest("namespace tool missing tools".to_owned())
                    })?;
                for nested in nested_tools {
                    let (converted, openai_name) = convert_function_tool(nested, Some(namespace))?;
                    let sanitized = converted["function"]["name"]
                        .as_str()
                        .expect("converted namespace tool missing name")
                        .to_owned();
                    tool_names.insert(sanitized, openai_name);
                    result.push(converted);
                }
            }
            Some("web_search") => {}
            Some(other) => tracing::debug!(tool_type = other, "skipping unsupported tool type"),
            None => {}
        }
    }
    Ok((result, tool_names))
}

fn convert_function_tool(
    tool: &Value,
    namespace: Option<&str>,
) -> Result<(Value, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("function tool missing name".to_owned()))?;
    let openai_name = namespace.map_or_else(
        || name.to_owned(),
        |namespace| format!("{namespace}.{name}"),
    );
    let sanitized_name = namespace.map_or_else(
        || sanitize_tool_name(name),
        |namespace| sanitize_tool_name(&format!("{namespace}__{name}")),
    );
    let description = tool
        .get("description")
        .cloned()
        .unwrap_or_else(|| json!(""));
    let parameters = tool
        .get("parameters")
        .or_else(|| tool.get("input_schema"))
        .cloned()
        .unwrap_or_else(|| json!({"type":"object","properties":{}}));
    Ok((
        json!({
            "type": "function",
            "function": {
                "name": sanitized_name,
                "description": description,
                "parameters": parameters
            }
        }),
        openai_name,
    ))
}
