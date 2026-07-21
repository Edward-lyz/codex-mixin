use super::*;

pub(super) fn append_input_item(
    item: &Value,
    messages: &mut Vec<Value>,
) -> Result<(), GatewayError> {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("input item missing type".to_owned()))?;
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
            let upstream_name = item.get("namespace").and_then(Value::as_str).map_or_else(
                || name.to_owned(),
                |namespace| format!("{namespace}__{name}"),
            );
            let arguments = match item.get("arguments") {
                Some(Value::String(arguments)) => arguments.clone(),
                Some(Value::Object(_)) => serde_json::to_string(&item["arguments"])?,
                Some(Value::Null) | None => {
                    return Err(GatewayError::BadRequest(
                        "function_call missing arguments".to_owned(),
                    ));
                }
                Some(other) => {
                    return Err(GatewayError::BadRequest(format!(
                        "function_call arguments must be a JSON string or object, got {other}"
                    )));
                }
            };
            append_chat_tool_call(
                messages,
                json!({
                    "id": call_id,
                    "type": "function",
                    "function": {"name": sanitize_tool_name(&upstream_name), "arguments": arguments}
                }),
            );
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("function_call_output missing call_id".to_owned())
            })?;
            let output = tool_output_for_openai_chat(item.get("output"))?;
            messages.push(json!({"role":"tool","tool_call_id":call_id,"content":output}));
        }
        "custom_tool_call" => {
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("custom_tool_call missing call_id".to_owned())
            })?;
            let name = item.get("name").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("custom_tool_call missing name".to_owned())
            })?;
            let input = item.get("input").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("custom_tool_call missing input".to_owned())
            })?;
            append_chat_tool_call(
                messages,
                json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": sanitize_tool_name(name),
                        "arguments": serde_json::to_string(&json!({"input": input}))?
                    }
                }),
            );
        }
        "tool_search_call" => {
            let execution = item
                .get("execution")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    GatewayError::BadRequest("tool_search_call missing execution".to_owned())
                })?;
            if execution == "server" {
                return Ok(());
            }
            if execution != "client" {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported tool_search_call execution: {execution}"
                )));
            }
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("tool_search_call missing call_id".to_owned())
            })?;
            let arguments = item
                .get("arguments")
                .filter(|arguments| arguments.is_object())
                .ok_or_else(|| {
                    GatewayError::BadRequest(
                        "tool_search_call arguments must be an object".to_owned(),
                    )
                })?;
            append_chat_tool_call(
                messages,
                json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": "tool_search",
                        "arguments": serde_json::to_string(&arguments)?
                    }
                }),
            );
        }
        "tool_search_output" => {
            let execution = item
                .get("execution")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    GatewayError::BadRequest("tool_search_output missing execution".to_owned())
                })?;
            if execution == "server" {
                return Ok(());
            }
            if execution != "client" {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported tool_search_output execution: {execution}"
                )));
            }
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("tool_search_output missing call_id".to_owned())
            })?;
            let tools = item.get("tools").and_then(Value::as_array).ok_or_else(|| {
                GatewayError::BadRequest("tool_search_output missing tools".to_owned())
            })?;
            let output = serde_json::to_string(tools)?;
            messages.push(json!({"role":"tool","tool_call_id":call_id,"content":output}));
        }
        "reasoning" | "web_search_call" | "image_generation_call" | "additional_tools" => {}
        "agent_message" => {
            messages.push(json!({"role":"user","content":agent_message_text(item)?}));
        }
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unsupported input item type: {other}"
            )));
        }
    }
    Ok(())
}

pub(super) fn append_chat_tool_call(messages: &mut Vec<Value>, tool_call: Value) {
    if let Some(tool_calls) = messages.last_mut().and_then(|message| {
        (message.get("role").and_then(Value::as_str) == Some("assistant")
            && message.get("content").is_some_and(Value::is_null))
        .then(|| message.get_mut("tool_calls").and_then(Value::as_array_mut))
        .flatten()
    }) {
        tool_calls.push(tool_call);
        return;
    }
    messages.push(json!({
        "role":"assistant",
        "content":null,
        "tool_calls":[tool_call]
    }));
}

pub(super) fn convert_content(content: Option<&Value>) -> Result<Value, GatewayError> {
    match content {
        Some(Value::String(text)) => Ok(json!(text)),
        Some(Value::Array(parts)) => {
            let mut converted = Vec::new();
            for part in parts {
                let part_type = part.get("type").and_then(Value::as_str).ok_or_else(|| {
                    GatewayError::BadRequest("message content part missing type".to_owned())
                })?;
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
        None => Err(GatewayError::BadRequest(
            "message missing content".to_owned(),
        )),
    }
}

pub(super) fn tool_output_for_openai_chat(output: Option<&Value>) -> Result<Value, GatewayError> {
    match output {
        Some(Value::String(output)) => Ok(Value::String(output.clone())),
        Some(Value::Array(items)) => {
            let mut text = Vec::with_capacity(items.len());
            let mut content = Vec::with_capacity(items.len());
            let mut has_image = false;
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("input_text") => {
                        let value = item.get("text").and_then(Value::as_str).ok_or_else(|| {
                            GatewayError::BadRequest(
                                "tool output input_text missing text".to_owned(),
                            )
                        })?;
                        text.push(value);
                        content.push(json!({"type":"text","text":value}));
                    }
                    Some("input_image") => {
                        let image_url =
                            item.get("image_url")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    GatewayError::BadRequest(
                                        "tool output input_image missing image_url".to_owned(),
                                    )
                                })?;
                        has_image = true;
                        content.push(json!({"type":"image_url","image_url":{"url":image_url}}));
                    }
                    Some("encrypted_content") => {
                        return Err(GatewayError::BadRequest(
                            "encrypted tool output cannot be forwarded to a custom upstream"
                                .to_owned(),
                        ));
                    }
                    Some(other) => {
                        return Err(GatewayError::BadRequest(format!(
                            "unsupported tool output content type: {other}"
                        )));
                    }
                    None => {
                        return Err(GatewayError::BadRequest(
                            "tool output content item missing type".to_owned(),
                        ));
                    }
                }
            }
            if has_image {
                Ok(Value::Array(content))
            } else {
                Ok(Value::String(text.join("\n")))
            }
        }
        Some(Value::Null) | None => Err(GatewayError::BadRequest(
            "tool call output is missing".to_owned(),
        )),
        Some(_) => Err(GatewayError::BadRequest(
            "tool call output must be a string or content array".to_owned(),
        )),
    }
}
