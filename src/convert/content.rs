use super::tools::upstream_client_tool_name;
use super::*;

pub(super) fn append_input_item(
    item: &Value,
    system: &mut Vec<ContentBlock>,
    messages: &mut Vec<Message>,
    use_mcp_bridge_names: bool,
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
            let content = convert_content(item.get("content"), role)?;
            match role {
                "developer" | "system" => system.extend(content),
                "user" | "assistant" => messages.push(Message {
                    role: role.to_owned(),
                    content,
                }),
                other => {
                    return Err(GatewayError::BadRequest(format!(
                        "unsupported message role: {other}"
                    )));
                }
            }
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
            let input = match item.get("arguments") {
                Some(Value::String(arguments)) => {
                    serde_json::from_str(arguments).map_err(|err| {
                        GatewayError::BadRequest(format!(
                            "function_call arguments are not JSON: {err}"
                        ))
                    })?
                }
                Some(Value::Object(_)) => item["arguments"].clone(),
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
            messages.push(Message {
                role: "assistant".to_owned(),
                content: vec![ContentBlock::ToolUse {
                    id: call_id.to_owned(),
                    name: upstream_client_tool_name(&upstream_name, use_mcp_bridge_names),
                    input,
                }],
            });
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("function_call_output missing call_id".to_owned())
            })?;
            let output = tool_output_for_anthropic(item.get("output"))?;
            messages.push(Message {
                role: "user".to_owned(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: call_id.to_owned(),
                    content: output,
                }],
            });
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
            messages.push(Message {
                role: "assistant".to_owned(),
                content: vec![ContentBlock::ToolUse {
                    id: call_id.to_owned(),
                    name: upstream_client_tool_name(name, use_mcp_bridge_names),
                    input: json!({"input": input}),
                }],
            });
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
            messages.push(Message {
                role: "assistant".to_owned(),
                content: vec![ContentBlock::ToolUse {
                    id: call_id.to_owned(),
                    name: upstream_client_tool_name("tool_search", use_mcp_bridge_names),
                    input: arguments.clone(),
                }],
            });
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
            messages.push(Message {
                role: "user".to_owned(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: call_id.to_owned(),
                    content: Value::String(serde_json::to_string(tools)?),
                }],
            });
        }
        "reasoning" | "web_search_call" | "image_generation_call" | "additional_tools" => {}
        "agent_message" => messages.push(Message {
            role: "user".to_owned(),
            content: vec![ContentBlock::Text {
                text: agent_message_text(item)?,
            }],
        }),
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unsupported input item type: {other}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn agent_message_text(item: &Value) -> Result<String, GatewayError> {
    let author = item
        .get("author")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("agent_message missing author".to_owned()))?;
    let recipient = item
        .get("recipient")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("agent_message missing recipient".to_owned()))?;
    let content = item
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| GatewayError::BadRequest("agent_message missing content".to_owned()))?;
    let mut text_parts = Vec::with_capacity(content.len());
    for part in content {
        match part.get("type").and_then(Value::as_str) {
            Some("input_text") => {
                text_parts.push(
                    part.get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            GatewayError::BadRequest(
                                "agent_message input_text missing text".to_owned(),
                            )
                        })?
                        .to_owned(),
                );
            }
            // Codex v2 stores the local collaboration tool's message argument in this field.
            // Third-party upstreams do not understand the Responses envelope, so materialize it.
            Some("encrypted_content") => text_parts.push(
                part.get("encrypted_content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        GatewayError::BadRequest(
                            "agent_message encrypted_content missing payload".to_owned(),
                        )
                    })?
                    .to_owned(),
            ),
            Some(other) => {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported agent_message content type: {other}"
                )));
            }
            None => {
                return Err(GatewayError::BadRequest(
                    "agent_message content missing type".to_owned(),
                ));
            }
        }
    }
    if text_parts.is_empty() {
        return Err(GatewayError::BadRequest(
            "agent_message has no plaintext content".to_owned(),
        ));
    }
    Ok(format!(
        "[Agent message from {author} to {recipient}]\n{}",
        text_parts.join("\n")
    ))
}

pub(super) fn convert_content(
    content: Option<&Value>,
    role: &str,
) -> Result<Vec<ContentBlock>, GatewayError> {
    match content {
        Some(Value::String(text)) => Ok(vec![ContentBlock::Text { text: text.clone() }]),
        Some(Value::Array(parts)) => {
            let mut blocks = Vec::new();
            for part in parts {
                let part_type = part.get("type").and_then(Value::as_str).ok_or_else(|| {
                    GatewayError::BadRequest("message content part missing type".to_owned())
                })?;
                match part_type {
                    "input_text" | "output_text" | "text" => {
                        let text = part.get("text").and_then(Value::as_str).ok_or_else(|| {
                            GatewayError::BadRequest(format!("{part_type} missing text"))
                        })?;
                        blocks.push(ContentBlock::Text {
                            text: text.to_owned(),
                        });
                    }
                    "input_image" => blocks.push(convert_image_part(part)?),
                    other => {
                        return Err(GatewayError::BadRequest(format!(
                            "unsupported content part type for {role}: {other}"
                        )));
                    }
                }
            }
            Ok(blocks)
        }
        Some(_) => Err(GatewayError::BadRequest(
            "message content must be a string or array".to_owned(),
        )),
        None => Err(GatewayError::BadRequest(
            "message missing content".to_owned(),
        )),
    }
}

pub(super) fn convert_image_part(part: &Value) -> Result<ContentBlock, GatewayError> {
    let image_url = part
        .get("image_url")
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("url").and_then(Value::as_str))
        })
        .ok_or_else(|| GatewayError::BadRequest("input_image missing image_url".to_owned()))?;
    if let Some(data_url) = image_url.strip_prefix("data:") {
        let (media_type, data) = data_url.split_once(";base64,").ok_or_else(|| {
            GatewayError::BadRequest("input_image data URL must be base64".to_owned())
        })?;
        return Ok(ContentBlock::Image {
            source: json!({"type": "base64", "media_type": media_type, "data": data}),
        });
    }
    Ok(ContentBlock::Image {
        source: json!({"type": "url", "url": image_url}),
    })
}

pub(super) fn merge_consecutive_messages(messages: &mut Vec<Message>) {
    let mut merged: Vec<Message> = Vec::with_capacity(messages.len());
    for message in messages.drain(..) {
        if let Some(last) = merged.last_mut()
            && last.role == message.role
        {
            last.content.extend(message.content);
            continue;
        }
        merged.push(message);
    }
    *messages = merged;
}

pub(crate) fn tool_output_for_anthropic(output: Option<&Value>) -> Result<Value, GatewayError> {
    match output {
        Some(Value::String(output)) => Ok(Value::String(output.clone())),
        Some(Value::Array(items)) => {
            let mut content = Vec::with_capacity(items.len());
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("input_text") => {
                        let text = item.get("text").and_then(Value::as_str).ok_or_else(|| {
                            GatewayError::BadRequest(
                                "tool output input_text missing text".to_owned(),
                            )
                        })?;
                        content.push(json!({"type":"text","text":text}));
                    }
                    Some("input_image") => {
                        content.push(serde_json::to_value(convert_image_part(item)?)?);
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
            Ok(Value::Array(content))
        }
        Some(Value::Null) | None => Err(GatewayError::BadRequest(
            "tool call output is missing".to_owned(),
        )),
        Some(_) => Err(GatewayError::BadRequest(
            "tool call output must be a string or content array".to_owned(),
        )),
    }
}
