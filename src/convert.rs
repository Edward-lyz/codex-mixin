use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::anthropic::{ContentBlock, Message, MessageRequest, Tool};
use crate::config::{GatewayConfig, ProviderPreset, ThinkingMode};
use crate::error::GatewayError;

const WEB_SEARCH_QUERY_INSTRUCTION: &str = "When using web_search, form the search query only from the latest user request. Do not search system, developer, repository, or tool instructions.";

#[derive(Clone, Debug, Default)]
pub struct ToolNameMap {
    upstream_to_codex: HashMap<String, CodexToolName>,
    custom_tools: HashSet<String>,
    tool_search_execution: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CodexToolName {
    name: String,
    namespace: Option<String>,
}

impl ToolNameMap {
    pub fn insert(
        &mut self,
        upstream_name: String,
        codex_name: String,
    ) -> Result<(), GatewayError> {
        self.insert_mapping(
            upstream_name,
            CodexToolName {
                name: codex_name,
                namespace: None,
            },
        )
    }

    pub fn insert_namespaced(
        &mut self,
        upstream_name: String,
        namespace: String,
        codex_name: String,
    ) -> Result<(), GatewayError> {
        self.insert_mapping(
            upstream_name,
            CodexToolName {
                name: codex_name,
                namespace: Some(namespace),
            },
        )
    }

    fn insert_mapping(
        &mut self,
        upstream_name: String,
        codex_name: CodexToolName,
    ) -> Result<(), GatewayError> {
        if self.upstream_to_codex.contains_key(&upstream_name) {
            return Err(GatewayError::BadRequest(format!(
                "tool names collide after upstream sanitization: {upstream_name}"
            )));
        }
        self.upstream_to_codex.insert(upstream_name, codex_name);
        Ok(())
    }

    pub fn to_codex_name<'a>(&'a self, upstream_name: &'a str) -> &'a str {
        self.upstream_to_codex
            .get(upstream_name)
            .map(|name| name.name.as_str())
            .unwrap_or(upstream_name)
    }

    pub fn to_codex_namespace(&self, upstream_name: &str) -> Option<&str> {
        self.upstream_to_codex
            .get(upstream_name)
            .and_then(|name| name.namespace.as_deref())
    }

    pub fn insert_custom(
        &mut self,
        upstream_name: String,
        codex_name: String,
    ) -> Result<(), GatewayError> {
        self.insert(upstream_name.clone(), codex_name)?;
        self.custom_tools.insert(upstream_name.clone());
        Ok(())
    }

    pub fn is_custom(&self, upstream_name: &str) -> bool {
        self.custom_tools.contains(upstream_name)
    }

    pub fn insert_tool_search(
        &mut self,
        upstream_name: String,
        codex_name: String,
        execution: String,
    ) -> Result<(), GatewayError> {
        self.insert(upstream_name.clone(), codex_name)?;
        self.tool_search_execution
            .insert(upstream_name.clone(), execution);
        Ok(())
    }

    pub fn tool_search_execution(&self, upstream_name: &str) -> Option<&str> {
        self.tool_search_execution
            .get(upstream_name)
            .map(String::as_str)
    }
}

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
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let use_mcp_bridge_names = config.provider_preset == ProviderPreset::BaiduOneApi
        && model.to_ascii_lowercase().contains("fable");
    let web_search_turn =
        config.enable_web_search_tool && request_has_codex_web_search_tool(body.get("tools"));
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
    if web_search_turn && config.web_search_latest_user_only {
        let latest_user_message = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .cloned()
            .ok_or_else(|| {
                GatewayError::BadRequest(
                    "web_search request has no user message to search from".to_owned(),
                )
            })?;
        messages.clear();
        messages.push(latest_user_message);
    }
    merge_consecutive_messages(&mut messages);

    let active_tools = collect_active_tools(body)?;
    let (tools, tool_names) = convert_tools(Some(&active_tools), config, use_mcp_bridge_names)?;
    if tools.iter().any(is_anthropic_web_search_tool) {
        if config.web_search_omit_system_instructions {
            system.clear();
        }
        system.push(ContentBlock::Text {
            text: WEB_SEARCH_QUERY_INSTRUCTION.to_owned(),
        });
    }
    let thinking = convert_thinking(&model, max_tokens, body.get("reasoning"), config)?;
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
            messages,
            system: if system.is_empty() {
                None
            } else {
                Some(system)
            },
            tools,
            tool_choice,
            thinking: thinking.thinking,
            output_config: thinking.output_config,
            metadata: None,
        },
        tool_names,
    })
}

fn append_input_item(
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

fn convert_content(content: Option<&Value>, role: &str) -> Result<Vec<ContentBlock>, GatewayError> {
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

fn convert_image_part(part: &Value) -> Result<ContentBlock, GatewayError> {
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

fn merge_consecutive_messages(messages: &mut Vec<Message>) {
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

pub(crate) fn collect_active_tools(body: &Value) -> Result<Value, GatewayError> {
    let mut active_tools = match body.get("tools") {
        Some(Value::Array(tools)) => tools.clone(),
        Some(_) => {
            return Err(GatewayError::BadRequest(
                "tools must be an array".to_owned(),
            ));
        }
        None => Vec::new(),
    };
    let Some(Value::Array(input)) = body.get("input") else {
        return Ok(Value::Array(active_tools));
    };
    for item in input {
        let item_type = item.get("type").and_then(Value::as_str);
        let discovered_tools = match item_type {
            Some("tool_search_output")
                if item.get("execution").and_then(Value::as_str) == Some("client") =>
            {
                item.get("tools").and_then(Value::as_array).ok_or_else(|| {
                    GatewayError::BadRequest("tool_search_output missing tools".to_owned())
                })?
            }
            Some("additional_tools") => {
                item.get("tools").and_then(Value::as_array).ok_or_else(|| {
                    GatewayError::BadRequest("additional_tools missing tools".to_owned())
                })?
            }
            _ => continue,
        };
        // Codex retains every tool search result, and overlapping searches can
        // return the same definition again in later turns.
        for tool in discovered_tools {
            if tool.get("type").and_then(Value::as_str) == Some("namespace") {
                let namespace = tool.get("name").and_then(Value::as_str).ok_or_else(|| {
                    GatewayError::BadRequest("namespace tool missing name".to_owned())
                })?;
                if let Some(existing_index) = active_tools.iter().position(|existing| {
                    existing.get("type").and_then(Value::as_str) == Some("namespace")
                        && existing.get("name").and_then(Value::as_str) == Some(namespace)
                }) {
                    let mut existing_metadata = active_tools[existing_index].clone();
                    existing_metadata
                        .as_object_mut()
                        .expect("namespace tool must be an object")
                        .remove("tools");
                    let mut discovered_metadata = tool.clone();
                    discovered_metadata
                        .as_object_mut()
                        .expect("namespace tool must be an object")
                        .remove("tools");
                    if existing_metadata != discovered_metadata {
                        return Err(GatewayError::BadRequest(format!(
                            "conflicting namespace definitions across tool search history: {namespace}"
                        )));
                    }

                    let discovered_nested =
                        tool.get("tools").and_then(Value::as_array).ok_or_else(|| {
                            GatewayError::BadRequest("namespace tool missing tools".to_owned())
                        })?;
                    let existing_nested = active_tools[existing_index]
                        .get_mut("tools")
                        .and_then(Value::as_array_mut)
                        .ok_or_else(|| {
                            GatewayError::BadRequest("namespace tool missing tools".to_owned())
                        })?;
                    for nested in discovered_nested {
                        let nested_name =
                            nested.get("name").and_then(Value::as_str).ok_or_else(|| {
                                GatewayError::BadRequest(
                                    "namespace function tool missing name".to_owned(),
                                )
                            })?;
                        if let Some(existing) = existing_nested.iter().find(|existing| {
                            existing.get("name").and_then(Value::as_str) == Some(nested_name)
                        }) {
                            if existing != nested {
                                return Err(GatewayError::BadRequest(format!(
                                    "conflicting definitions for discovered tool: {namespace}.{nested_name}"
                                )));
                            }
                        } else {
                            existing_nested.push(nested.clone());
                        }
                    }
                    continue;
                }
            }
            if !active_tools.contains(tool) {
                active_tools.push(tool.clone());
            }
        }
    }
    Ok(Value::Array(active_tools))
}

fn convert_tools(
    tools: Option<&Value>,
    config: &GatewayConfig,
    use_mcp_bridge_names: bool,
) -> Result<(Vec<Tool>, ToolNameMap), GatewayError> {
    let mut result = Vec::new();
    let mut names = ToolNameMap::default();
    let Some(Value::Array(tools)) = tools else {
        return Ok((result, names));
    };
    let web_search_requested = tools.iter().any(is_codex_web_search_tool);
    let suppress_client_tools =
        config.enable_web_search_tool && config.web_search_exclusive && web_search_requested;
    let mut web_search_added = false;
    for tool in tools {
        match tool.get("type").and_then(Value::as_str) {
            Some("function")
                if is_codex_web_search_function(tool)
                    && config.enable_web_search_tool
                    && !web_search_added =>
            {
                result.push(web_search_server_tool(config, tool)?);
                web_search_added = true;
            }
            Some("function")
                if is_codex_web_search_function(tool) && config.enable_web_search_tool => {}
            Some("function") if is_codex_web_search_function(tool) => {
                tracing::debug!("omitting unavailable hosted web_search tool");
            }
            Some("function") if suppress_client_tools => {}
            Some("function") => {
                let (converted, codex_name) =
                    convert_function_tool(tool, None, use_mcp_bridge_names)?;
                let anthropic_name = converted
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("converted function tool missing name")
                    .to_owned();
                names.insert(anthropic_name, codex_name)?;
                result.push(converted);
            }
            Some("namespace") if suppress_client_tools => {}
            Some("namespace") => {
                let namespace = tool.get("name").and_then(Value::as_str).ok_or_else(|| {
                    GatewayError::BadRequest("namespace tool missing name".to_owned())
                })?;
                let nested_tools =
                    tool.get("tools").and_then(Value::as_array).ok_or_else(|| {
                        GatewayError::BadRequest("namespace tool missing tools".to_owned())
                    })?;
                for nested in nested_tools {
                    let (converted, codex_name) =
                        convert_function_tool(nested, Some(namespace), use_mcp_bridge_names)?;
                    let anthropic_name = converted
                        .get("name")
                        .and_then(Value::as_str)
                        .expect("converted namespace tool missing name")
                        .to_owned();
                    names.insert_namespaced(anthropic_name, namespace.to_owned(), codex_name)?;
                    result.push(converted);
                }
            }
            Some("custom") if suppress_client_tools => {}
            Some("custom") => {
                let (converted, codex_name) = convert_custom_tool(tool, use_mcp_bridge_names)?;
                let upstream_name = converted
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("converted custom tool missing name")
                    .to_owned();
                names.insert_custom(upstream_name, codex_name)?;
                result.push(converted);
            }
            Some("tool_search") if suppress_client_tools => {}
            Some("tool_search") => {
                let execution = tool
                    .get("execution")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        GatewayError::BadRequest("tool_search missing execution".to_owned())
                    })?
                    .to_owned();
                if execution != "client" {
                    return Err(GatewayError::BadRequest(format!(
                        "unsupported tool_search execution: {execution}"
                    )));
                }
                let mut function_tool = tool.clone();
                function_tool["name"] = json!("tool_search");
                let (converted, codex_name) =
                    convert_function_tool(&function_tool, None, use_mcp_bridge_names)?;
                let upstream_name = converted
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("converted tool_search missing name")
                    .to_owned();
                names.insert_tool_search(upstream_name, codex_name, execution)?;
                result.push(converted);
            }
            Some("web_search" | "web_search_preview")
                if config.enable_web_search_tool && !web_search_added =>
            {
                result.push(web_search_server_tool(config, tool)?);
                web_search_added = true;
            }
            Some("web_search" | "web_search_preview") if config.enable_web_search_tool => {}
            Some("web_search" | "web_search_preview") => {
                tracing::debug!("omitting unavailable hosted web_search tool");
            }
            Some("image_generation") => {
                tracing::debug!("omitting legacy OpenAI-hosted image_generation tool");
            }
            Some(other) => {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported tool type: {other}"
                )));
            }
            None => {
                return Err(GatewayError::BadRequest(
                    "tool definition missing type".to_owned(),
                ));
            }
        }
    }
    Ok((result, names))
}

fn request_has_codex_web_search_tool(tools: Option<&Value>) -> bool {
    let Some(Value::Array(tools)) = tools else {
        return false;
    };
    tools.iter().any(is_codex_web_search_tool)
}

fn is_codex_web_search_tool(tool: &Value) -> bool {
    matches!(
        tool.get("type").and_then(Value::as_str),
        Some("web_search" | "web_search_preview")
    ) || is_codex_web_search_function(tool)
}

fn is_anthropic_web_search_tool(tool: &Tool) -> bool {
    tool.get("name").and_then(Value::as_str) == Some("web_search")
        && tool
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|tool_type| tool_type.starts_with("web_search_"))
}

fn is_codex_web_search_function(tool: &Value) -> bool {
    matches!(
        tool.get("name").and_then(Value::as_str),
        Some("web_search" | "web_search_preview")
    )
}

fn web_search_server_tool(
    config: &GatewayConfig,
    codex_tool: &Value,
) -> Result<Tool, GatewayError> {
    if codex_tool
        .get("external_web_access")
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Err(GatewayError::BadRequest(
            "Anthropic web_search cannot preserve Codex cached-search semantics".to_owned(),
        ));
    }
    if codex_tool
        .get("indexed_web_access")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Err(GatewayError::BadRequest(
            "Anthropic web_search cannot preserve Codex indexed-search semantics".to_owned(),
        ));
    }
    if codex_tool.get("search_context_size").is_some() {
        return Err(GatewayError::BadRequest(
            "Anthropic web_search has no equivalent for Codex search_context_size".to_owned(),
        ));
    }
    if let Some(content_types) = codex_tool.get("search_content_types") {
        let content_types = content_types.as_array().ok_or_else(|| {
            GatewayError::BadRequest("web_search search_content_types must be an array".to_owned())
        })?;
        if content_types
            .iter()
            .any(|content_type| content_type.as_str() != Some("text"))
        {
            return Err(GatewayError::BadRequest(
                "Anthropic web_search cannot preserve non-text Codex search content types"
                    .to_owned(),
            ));
        }
    }

    let mut web_search_tool = json!({"type": &config.web_search_tool_type, "name": "web_search"});
    if let Some(max_uses) = config.web_search_max_uses {
        web_search_tool["max_uses"] = json!(max_uses);
    }
    if let Some(filters) = codex_tool.get("filters") {
        let filters = filters.as_object().ok_or_else(|| {
            GatewayError::BadRequest("web_search filters must be an object".to_owned())
        })?;
        if let Some(allowed_domains) = filters.get("allowed_domains") {
            let allowed_domains = allowed_domains.as_array().ok_or_else(|| {
                GatewayError::BadRequest(
                    "web_search filters.allowed_domains must be an array".to_owned(),
                )
            })?;
            if allowed_domains.iter().any(|domain| !domain.is_string()) {
                return Err(GatewayError::BadRequest(
                    "web_search filters.allowed_domains must contain only strings".to_owned(),
                ));
            }
            web_search_tool["allowed_domains"] = Value::Array(allowed_domains.clone());
        }
    }
    if let Some(user_location) = codex_tool.get("user_location") {
        if !user_location.is_object() {
            return Err(GatewayError::BadRequest(
                "web_search user_location must be an object".to_owned(),
            ));
        }
        web_search_tool["user_location"] = user_location.clone();
    }
    Ok(web_search_tool)
}

fn convert_function_tool(
    tool: &Value,
    namespace: Option<&str>,
    use_mcp_bridge_names: bool,
) -> Result<(Tool, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("function tool missing name".to_owned()))?;
    let codex_name = name.to_owned();
    let anthropic_name = namespace.map_or_else(
        || upstream_client_tool_name(name, use_mcp_bridge_names),
        |namespace| {
            upstream_client_tool_name(&format!("{namespace}__{name}"), use_mcp_bridge_names)
        },
    );
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| namespace.map(|namespace| format!("Tool {name} in namespace {namespace}.")));
    let input_schema = tool
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
    let mut converted = match description {
        Some(description) => json!({
            "name": anthropic_name,
            "description": description,
            "input_schema": input_schema
        }),
        None => json!({
            "name": anthropic_name,
            "input_schema": input_schema
        }),
    };
    if let Some(strict) = tool.get("strict").and_then(Value::as_bool) {
        converted["strict"] = json!(strict);
    }
    Ok((converted, codex_name))
}

fn convert_custom_tool(
    tool: &Value,
    use_mcp_bridge_names: bool,
) -> Result<(Tool, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("custom tool missing name".to_owned()))?;
    let upstream_name = upstream_client_tool_name(name, use_mcp_bridge_names);
    let description = custom_tool_description(tool)?;
    Ok((
        json!({
            "name": upstream_name,
            "description": description,
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {"input": {"type": "string"}},
                "required": ["input"],
                "additionalProperties": false
            }
        }),
        name.to_owned(),
    ))
}

pub(crate) fn custom_tool_description(tool: &Value) -> Result<String, GatewayError> {
    let mut description = tool
        .get("description")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("custom tool missing description".to_owned()))?
        .to_owned();
    description.push_str(
        "\n\nFor this gateway function call, put the complete freeform payload unchanged in the input string field.",
    );
    if let Some(format) = tool.get("format") {
        let format_type = format.get("type").and_then(Value::as_str).ok_or_else(|| {
            GatewayError::BadRequest("custom tool format missing type".to_owned())
        })?;
        let syntax = format
            .get("syntax")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GatewayError::BadRequest("custom tool format missing syntax".to_owned())
            })?;
        let definition = format
            .get("definition")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GatewayError::BadRequest("custom tool format missing definition".to_owned())
            })?;
        description.push_str(&format!(
            "\nThe input must satisfy the {format_type} {syntax} grammar:\n{definition}"
        ));
    }
    Ok(description)
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

pub fn sanitize_tool_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() {
        sanitized.push_str("tool");
    }
    if sanitized.len() <= 64 {
        return sanitized;
    }
    let hash = name
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    sanitized.truncate(47);
    format!("{sanitized}_{hash:016x}")
}

fn upstream_client_tool_name(name: &str, use_mcp_bridge_names: bool) -> String {
    let sanitized = sanitize_tool_name(name);
    if !use_mcp_bridge_names || sanitized.starts_with("mcp__") {
        return sanitized;
    }
    sanitize_tool_name(&format!("mcp__codex__{sanitized}"))
}

fn convert_tool_choice(value: Option<&Value>, parallel_tool_calls: Option<bool>) -> Option<Value> {
    let mut choice = match value {
        Some(Value::String(choice)) if choice == "auto" => Some(json!({"type": "auto"})),
        Some(Value::String(choice)) if choice == "required" || choice == "any" => {
            Some(json!({"type": "any"}))
        }
        Some(Value::String(choice)) if choice == "none" => Some(json!({"type": "none"})),
        _ => None,
    };
    if parallel_tool_calls == Some(false)
        && !matches!(
            choice
                .as_ref()
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str),
            Some("none")
        )
    {
        let choice = choice.get_or_insert_with(|| json!({"type":"auto"}));
        choice["disable_parallel_tool_use"] = json!(true);
    }
    choice
}

#[derive(Clone, Debug, Default)]
struct ThinkingSettings {
    thinking: Option<Value>,
    output_config: Option<Value>,
}

fn convert_thinking(
    model: &str,
    max_tokens: u64,
    reasoning: Option<&Value>,
    config: &GatewayConfig,
) -> Result<ThinkingSettings, GatewayError> {
    let effort = reasoning
        .and_then(|value| value.get("effort"))
        .and_then(Value::as_str);
    if matches!(effort, Some("off" | "none" | "disabled")) {
        return Ok(ThinkingSettings::default());
    }
    match config.thinking_mode {
        ThinkingMode::Off => Ok(ThinkingSettings::default()),
        ThinkingMode::Manual => manual_thinking(max_tokens, effort),
        ThinkingMode::Adaptive => adaptive_thinking(effort),
        ThinkingMode::Auto if model_uses_adaptive_thinking(model) => adaptive_thinking(effort),
        ThinkingMode::Auto if model_uses_manual_thinking(model) => {
            manual_thinking(max_tokens, effort)
        }
        ThinkingMode::Auto => Ok(ThinkingSettings::default()),
    }
}

fn adaptive_thinking(effort: Option<&str>) -> Result<ThinkingSettings, GatewayError> {
    Ok(ThinkingSettings {
        thinking: Some(json!({"type": "adaptive", "display": "omitted"})),
        output_config: Some(json!({"effort": adaptive_effort(effort)?})),
    })
}

fn manual_thinking(
    max_tokens: u64,
    effort: Option<&str>,
) -> Result<ThinkingSettings, GatewayError> {
    if max_tokens <= 1024 {
        return Err(GatewayError::BadRequest(
            "manual Anthropic thinking requires max_output_tokens greater than 1024".to_owned(),
        ));
    }
    let budget_tokens = manual_budget_tokens(effort)?.min(max_tokens - 1);
    Ok(ThinkingSettings {
        thinking: Some(
            json!({"type": "enabled", "budget_tokens": budget_tokens, "display": "omitted"}),
        ),
        output_config: None,
    })
}

fn adaptive_effort(effort: Option<&str>) -> Result<&'static str, GatewayError> {
    match effort.unwrap_or("medium") {
        "minimal" | "low" => Ok("low"),
        "medium" => Ok("medium"),
        "high" => Ok("high"),
        "xhigh" | "exhigh" | "max" => Ok("max"),
        other => Err(GatewayError::BadRequest(format!(
            "unsupported reasoning effort for Anthropic adaptive thinking: {other}"
        ))),
    }
}

fn manual_budget_tokens(effort: Option<&str>) -> Result<u64, GatewayError> {
    match effort.unwrap_or("medium") {
        "minimal" | "low" => Ok(1024),
        "medium" => Ok(4096),
        "high" => Ok(8192),
        "xhigh" | "exhigh" | "max" => Ok(16_384),
        other => Err(GatewayError::BadRequest(format!(
            "unsupported reasoning effort for manual Anthropic thinking: {other}"
        ))),
    }
}

fn model_uses_adaptive_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    [
        "fable",
        "mythos",
        "sonnet 5",
        "sonnet-5",
        "sonnet_5",
        "sonnet 4.6",
        "sonnet-4-6",
        "sonnet_4_6",
        "opus 4.8",
        "opus-4-8",
        "opus_4_8",
        "opus 4.7",
        "opus-4-7",
        "opus_4_7",
        "opus 4.6",
        "opus-4-6",
        "opus_4_6",
    ]
    .iter()
    .any(|needle| model.contains(needle))
}

fn model_uses_manual_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    [
        "sonnet 3.7",
        "sonnet-3-7",
        "sonnet_3_7",
        "sonnet 4",
        "sonnet-4",
        "sonnet_4",
        "opus 4",
        "opus-4",
        "opus_4",
        "haiku 4.5",
        "haiku-4-5",
        "haiku_4_5",
    ]
    .iter()
    .any(|needle| model.contains(needle))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use crate::config::{
        GatewayConfig, ProviderPreset, ThinkingMode, UpstreamAuthHeader, UpstreamKind,
    };

    use super::*;

    fn config() -> GatewayConfig {
        GatewayConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            provider_preset: ProviderPreset::Custom,
            upstream_kind: UpstreamKind::AnthropicMessages,
            upstream_base_url: "http://127.0.0.1".to_owned(),
            upstream_messages_path: "/v1/messages".to_owned(),
            upstream_models_path: "/v1/models".to_owned(),
            upstream_image_generation_path: None,
            upstream_api_key: "test".to_owned(),
            quota_url: None,
            quota_username: None,
            official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
            codex_auth_path: std::path::PathBuf::from("/tmp/codex-auth.json"),
            upstream_auth_header: UpstreamAuthHeader::AuthorizationBearer,
            anthropic_version: "2023-06-01".to_owned(),
            anthropic_beta: None,
            gateway_api_key: None,
            accept_codex_oauth: true,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(30),
            thinking_mode: ThinkingMode::Off,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            web_search_exclusive: true,
            web_search_omit_system_instructions: true,
            web_search_latest_user_only: true,
        }
    }

    #[test]
    fn converts_codex_tool_loop_input() {
        let body = json!({
            "model": "DeepSeek-V4-Flash",
            "stream": true,
            "instructions": "base",
            "input": [
                {"type":"message","role":"developer","content":[{"type":"input_text","text":"dev"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"run pwd"}]},
                {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
                {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
            ],
            "tools": [{"type":"function","name":"exec_command","description":"run","parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}}]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(converted.request.model, "DeepSeek-V4-Flash");
        assert_eq!(converted.request.system.as_ref().unwrap().len(), 2);
        assert_eq!(converted.request.messages.len(), 3);
        assert_eq!(
            converted.request.messages[1].content[0],
            ContentBlock::ToolUse {
                id: "call_1".to_owned(),
                name: "exec_command".to_owned(),
                input: json!({"cmd":"pwd"})
            }
        );
        assert_eq!(
            converted.request.messages[2].content[0],
            ContentBlock::ToolResult {
                tool_use_id: "call_1".to_owned(),
                content: json!("/tmp")
            }
        );
        assert_eq!(converted.request.tools[0]["name"], "exec_command");
    }

    #[test]
    fn uses_mcp_tool_names_for_baidu_fable_bridge_and_history() {
        let mut config = config();
        config.provider_preset = ProviderPreset::BaiduOneApi;
        let body = json!({
            "model": "Fable 5",
            "stream": true,
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"run pwd"}]},
                {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
                {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
            ],
            "tools": [
                {"type":"function","name":"exec_command","parameters":{"type":"object"}},
                {"type":"namespace","name":"collaboration","tools":[
                    {"type":"function","name":"spawn_agent","parameters":{"type":"object"}}
                ]},
                {"type":"namespace","name":"mcp__fff","tools":[
                    {"type":"function","name":"find_files","parameters":{"type":"object"}}
                ]},
                {"type":"custom","name":"apply_patch","description":"Apply a patch"},
                {"type":"tool_search","execution":"client","description":"Search tools","parameters":{"type":"object"}}
            ]
        });

        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(
            converted.request.messages[1].content[0],
            ContentBlock::ToolUse {
                id: "call_1".to_owned(),
                name: "mcp__codex__exec_command".to_owned(),
                input: json!({"cmd":"pwd"})
            }
        );
        let tool_names = converted
            .request
            .tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            tool_names,
            vec![
                "mcp__codex__exec_command",
                "mcp__codex__collaboration__spawn_agent",
                "mcp__fff__find_files",
                "mcp__codex__apply_patch",
                "mcp__codex__tool_search"
            ]
        );
        assert_eq!(
            converted
                .tool_names
                .to_codex_name("mcp__codex__exec_command"),
            "exec_command"
        );
        assert_eq!(
            converted
                .tool_names
                .to_codex_namespace("mcp__codex__collaboration__spawn_agent"),
            Some("collaboration")
        );
        assert!(converted.tool_names.is_custom("mcp__codex__apply_patch"));
        assert_eq!(
            converted
                .tool_names
                .tool_search_execution("mcp__codex__tool_search"),
            Some("client")
        );
    }

    #[test]
    fn converts_plaintext_agent_message_for_subagents() {
        let body = json!({
            "model": "DeepSeek-V4-Flash",
            "stream": true,
            "input": [{
                "type": "agent_message",
                "author": "/root",
                "recipient": "/root/run_uname",
                "content": [{"type":"input_text","text":"Run uname -a"}]
            }]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(converted.request.messages[0].role, "user");
        assert_eq!(
            converted.request.messages[0].content[0],
            ContentBlock::Text {
                text: "[Agent message from /root to /root/run_uname]\nRun uname -a".to_owned()
            }
        );
    }

    #[test]
    fn materializes_v2_agent_message_payload_for_custom_upstream() {
        let body = json!({
            "model": "DeepSeek-V4-Flash",
            "stream": true,
            "input": [{
                "type": "agent_message",
                "author": "/root",
                "recipient": "/root/worker",
                "content": [
                    {"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},
                    {"type":"encrypted_content","encrypted_content":"Inspect the workspace"}
                ]
            }]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(
            converted.request.messages[0].content[0],
            ContentBlock::Text {
                text: "[Agent message from /root to /root/worker]\nMessage Type: NEW_TASK\nPayload:\n\nInspect the workspace".to_owned()
            }
        );
    }

    #[test]
    fn flattens_namespace_tools_for_anthropic() {
        let body = json!({
            "model": "m",
            "stream": true,
            "input": "hi",
            "tools": [{"type":"namespace","name":"mcp__node_repl","tools":[{"type":"function","name":"js","strict":true,"parameters":{"type":"object"}}]}]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(converted.request.tools[0]["name"], "mcp__node_repl__js");
        assert_eq!(converted.request.tools[0]["strict"], true);
        assert_eq!(
            converted.tool_names.to_codex_name("mcp__node_repl__js"),
            "js"
        );
        assert_eq!(
            converted
                .tool_names
                .to_codex_namespace("mcp__node_repl__js"),
            Some("mcp__node_repl")
        );
    }

    #[test]
    fn flattens_namespaced_function_call_history_for_upstream() {
        let body = json!({
            "model": "m",
            "stream": true,
            "input": [{
                "type":"function_call",
                "call_id":"call_1",
                "namespace":"collaboration",
                "name":"spawn_agent",
                "arguments":"{\"task_name\":\"test\"}"
            }]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(
            converted.request.messages[0].content[0],
            ContentBlock::ToolUse {
                id: "call_1".to_owned(),
                name: "collaboration__spawn_agent".to_owned(),
                input: json!({"task_name":"test"})
            }
        );
    }

    #[test]
    fn converts_custom_and_tool_search_tools_for_anthropic() {
        let body = json!({
            "model": "m",
            "stream": true,
            "input": "hi",
            "tools": [
                {
                    "type":"custom",
                    "name":"apply_patch",
                    "description":"Apply patch without JSON wrapping",
                    "format":{"type":"grammar","syntax":"lark","definition":"start: PATCH"}
                },
                {
                    "type":"tool_search",
                    "execution":"client",
                    "description":"Search deferred tools",
                    "parameters":{"type":"object","properties":{"query":{"type":"string"}}}
                }
            ]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(converted.request.tools[0]["name"], "apply_patch");
        assert_eq!(
            converted.request.tools[0]["input_schema"]["required"],
            json!(["input"])
        );
        assert_eq!(converted.request.tools[0]["strict"], true);
        assert!(
            converted.request.tools[0]["description"]
                .as_str()
                .unwrap()
                .contains("start: PATCH")
        );
        assert!(converted.tool_names.is_custom("apply_patch"));
        assert_eq!(converted.request.tools[1]["name"], "tool_search");
        assert_eq!(
            converted.tool_names.tool_search_execution("tool_search"),
            Some("client")
        );
    }

    #[test]
    fn converts_custom_and_tool_search_outputs_for_anthropic() {
        let body = json!({
            "model": "m",
            "stream": true,
            "input": [
                {"type":"custom_tool_call","call_id":"custom_1","name":"apply_patch","input":"*** Begin Patch"},
                {"type":"custom_tool_call_output","call_id":"custom_1","output":[{"type":"input_text","text":"Done"}]},
                {"type":"tool_search_call","call_id":"search_1","execution":"client","arguments":{"query":"calendar"}},
                {"type":"tool_search_output","call_id":"search_1","status":"completed","execution":"client","tools":[{"type":"function","name":"create_event"}]}
            ]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(converted.request.messages.len(), 4);
        assert_eq!(
            converted.request.messages[0].content[0],
            ContentBlock::ToolUse {
                id: "custom_1".to_owned(),
                name: "apply_patch".to_owned(),
                input: json!({"input":"*** Begin Patch"})
            }
        );
        assert_eq!(
            converted.request.messages[1].content[0],
            ContentBlock::ToolResult {
                tool_use_id: "custom_1".to_owned(),
                content: json!([{"type":"text","text":"Done"}])
            }
        );
        assert_eq!(
            converted.request.messages[2].content[0],
            ContentBlock::ToolUse {
                id: "search_1".to_owned(),
                name: "tool_search".to_owned(),
                input: json!({"query":"calendar"})
            }
        );
    }

    #[test]
    fn exposes_deferred_and_additional_tools_to_anthropic() {
        let body = json!({
            "model": "m",
            "stream": true,
            "tools": [{
                "type":"tool_search",
                "execution":"client",
                "description":"Search tools",
                "parameters":{"type":"object","properties":{"query":{"type":"string"}}}
            }],
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"create event"}]},
                {"type":"tool_search_call","call_id":"search_1","execution":"client","arguments":{"query":"calendar"}},
                {"type":"tool_search_output","call_id":"search_1","status":"completed","execution":"client","tools":[
                    {"type":"function","name":"calendar_create","description":"Create event","parameters":{"type":"object"}},
                    {"type":"namespace","name":"mcp__calendar","tools":[{"type":"function","name":"delete_event","parameters":{"type":"object"}}]}
                ]},
                {"type":"additional_tools","role":"developer","tools":[
                    {"type":"function","name":"calendar_list","description":"List events","parameters":{"type":"object"}}
                ]}
            ]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        let tool_names = converted
            .request
            .tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            tool_names,
            vec![
                "tool_search",
                "calendar_create",
                "mcp__calendar__delete_event",
                "calendar_list"
            ]
        );
        assert_eq!(
            converted
                .tool_names
                .to_codex_namespace("mcp__calendar__delete_event"),
            Some("mcp__calendar")
        );
    }

    #[test]
    fn merges_overlapping_namespace_tools_across_search_history() {
        let reset_tool = json!({
            "type": "function",
            "name": "js_reset",
            "description": "Reset the JavaScript kernel",
            "parameters": {"type": "object"}
        });
        let body = json!({
            "model": "m",
            "stream": true,
            "tools": [{
                "type": "tool_search",
                "execution": "client",
                "description": "Search tools",
                "parameters": {"type": "object", "properties": {"query": {"type": "string"}}}
            }],
            "input": [
                {
                    "type": "tool_search_call",
                    "call_id": "search_1",
                    "execution": "client",
                    "arguments": {"query": "computer-use control mac app"}
                },
                {
                    "type": "tool_search_output",
                    "call_id": "search_1",
                    "status": "completed",
                    "execution": "client",
                    "tools": [{
                        "type": "namespace",
                        "name": "mcp__node_repl",
                        "description": "Persistent JavaScript kernel",
                        "tools": [
                            reset_tool.clone(),
                            {
                                "type": "function",
                                "name": "js",
                                "parameters": {"type": "object"}
                            }
                        ]
                    }]
                },
                {
                    "type": "tool_search_call",
                    "call_id": "search_2",
                    "execution": "client",
                    "arguments": {"query": "computer-use screenshot click keyboard macOS"}
                },
                {
                    "type": "tool_search_output",
                    "call_id": "search_2",
                    "status": "completed",
                    "execution": "client",
                    "tools": [{
                        "type": "namespace",
                        "name": "mcp__node_repl",
                        "description": "Persistent JavaScript kernel",
                        "tools": [
                            reset_tool,
                            {
                                "type": "function",
                                "name": "js_add_node_module_dir",
                                "parameters": {"type": "object"}
                            }
                        ]
                    }]
                }
            ]
        });

        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(converted.request.tools.len(), 4);
        assert_eq!(converted.request.tools[0]["name"], "tool_search");
        assert_eq!(
            converted.request.tools[1]["name"],
            "mcp__node_repl__js_reset"
        );
        assert_eq!(converted.request.tools[2]["name"], "mcp__node_repl__js");
        assert_eq!(
            converted.request.tools[3]["name"],
            "mcp__node_repl__js_add_node_module_dir"
        );
    }

    #[test]
    fn rejects_messages_without_content() {
        let error = responses_to_anthropic(
            &json!({
                "model": "m",
                "stream": true,
                "input": [{"type":"message","role":"user"}]
            }),
            &config(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("message missing content"));
    }

    #[test]
    fn rejects_conflicting_namespace_tools_across_search_history() {
        let body = json!({
            "input": [
                {
                    "type": "tool_search_output",
                    "execution": "client",
                    "tools": [{
                        "type": "namespace",
                        "name": "mcp__calendar",
                        "tools": [{
                            "type": "function",
                            "name": "create_event",
                            "parameters": {"type": "object"}
                        }]
                    }]
                },
                {
                    "type": "tool_search_output",
                    "execution": "client",
                    "tools": [{
                        "type": "namespace",
                        "name": "mcp__calendar",
                        "tools": [{
                            "type": "function",
                            "name": "create_event",
                            "parameters": {"type": "object", "required": ["title"]}
                        }]
                    }]
                }
            ]
        });

        let error = collect_active_tools(&body).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("conflicting definitions for discovered tool")
        );
    }

    #[test]
    fn ignores_provider_native_history_when_switching_to_custom_model() {
        let body = json!({
            "model": "m",
            "stream": true,
            "input": [
                {"type":"reasoning","encrypted_content":"opaque","summary":[]},
                {"type":"web_search_call","id":"ws_1","status":"completed"},
                {"type":"image_generation_call","id":"ig_1","status":"completed","result":"base64"},
                {"type":"tool_search_call","execution":"server","call_id":null,"arguments":{"paths":["crm"]}},
                {"type":"tool_search_output","execution":"server","call_id":null,"status":"completed","tools":[]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
            ]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(converted.request.messages.len(), 1);
        assert_eq!(
            converted.request.messages[0].content[0],
            ContentBlock::Text {
                text: "continue".to_owned()
            }
        );
    }

    #[test]
    fn preserves_multimodal_tool_outputs_for_anthropic() {
        let body = json!({
            "model": "m",
            "stream": true,
            "input": [
                {"type":"function_call","call_id":"call_1","name":"view_image","arguments":"{\"path\":\"/tmp/a.png\"}"},
                {"type":"function_call_output","call_id":"call_1","output":[
                    {"type":"input_text","text":"image loaded"},
                    {"type":"input_image","image_url":"data:image/png;base64,AAAA","detail":"original"}
                ]}
            ]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(
            converted.request.messages[1].content[0],
            ContentBlock::ToolResult {
                tool_use_id: "call_1".to_owned(),
                content: json!([
                    {"type":"text","text":"image loaded"},
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}
                ])
            }
        );
    }

    #[test]
    fn rejects_incomplete_client_tool_history() {
        for item in [
            json!({"type":"function_call","call_id":"call_1","name":"exec_command"}),
            json!({"type":"function_call_output","call_id":"call_1"}),
            json!({"type":"tool_search_call","execution":"client","call_id":"search_1"}),
            json!({"type":"tool_search_output","execution":"client","call_id":"search_1","status":"completed"}),
        ] {
            let error = responses_to_anthropic(
                &json!({"model":"m","stream":true,"input":[item]}),
                &config(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("missing") || error.to_string().contains("must be"));
        }
    }

    #[test]
    fn auto_thinking_maps_exhigh_to_adaptive_max_for_new_claude() {
        let mut config = config();
        config.thinking_mode = ThinkingMode::Auto;
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "max_output_tokens": 64,
            "reasoning": {"effort": "exhigh"},
            "input": "hi"
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(converted.request.thinking.unwrap()["type"], "adaptive");
        assert_eq!(converted.request.output_config.unwrap()["effort"], "max");
    }

    #[test]
    fn adaptive_thinking_maps_common_effort_levels() {
        let mut config = config();
        config.thinking_mode = ThinkingMode::Adaptive;
        for (input, expected) in [
            ("minimal", "low"),
            ("low", "low"),
            ("medium", "medium"),
            ("high", "high"),
            ("xhigh", "max"),
            ("exhigh", "max"),
            ("max", "max"),
        ] {
            let body = json!({
                "model": "Claude Sonnet 5",
                "stream": true,
                "reasoning": {"effort": input},
                "input": "hi"
            });
            let converted = responses_to_anthropic(&body, &config).unwrap();
            assert_eq!(
                converted.request.output_config.unwrap()["effort"],
                expected,
                "input effort {input}"
            );
        }
    }

    #[test]
    fn adaptive_thinking_rejects_misspelled_medium() {
        let mut config = config();
        config.thinking_mode = ThinkingMode::Adaptive;
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "reasoning": {"effort": "meddium"},
            "input": "hi"
        });
        let err = responses_to_anthropic(&body, &config).unwrap_err();
        assert!(err.to_string().contains("unsupported reasoning effort"));
    }

    #[test]
    fn adaptive_thinking_rejects_unknown_effort() {
        let mut config = config();
        config.thinking_mode = ThinkingMode::Adaptive;
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "reasoning": {"effort": "turbo"},
            "input": "hi"
        });
        let err = responses_to_anthropic(&body, &config).unwrap_err();
        assert!(err.to_string().contains("unsupported reasoning effort"));
    }

    #[test]
    fn auto_thinking_leaves_non_claude_models_untouched() {
        let mut config = config();
        config.thinking_mode = ThinkingMode::Auto;
        let body = json!({
            "model": "DeepSeek-V4-Flash",
            "stream": true,
            "reasoning": {"effort": "xhigh"},
            "input": "hi"
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert!(converted.request.thinking.is_none());
        assert!(converted.request.output_config.is_none());
    }

    #[test]
    fn manual_thinking_caps_budget_below_max_tokens() {
        let mut config = config();
        config.thinking_mode = ThinkingMode::Manual;
        let body = json!({
            "model": "Claude Haiku 4.5",
            "stream": true,
            "max_output_tokens": 2048,
            "reasoning": {"effort": "high"},
            "input": "hi"
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        let thinking = converted.request.thinking.unwrap();
        assert_eq!(thinking["type"], "enabled");
        assert_eq!(thinking["budget_tokens"], 2047);
        assert!(converted.request.output_config.is_none());
    }

    #[test]
    fn web_search_becomes_anthropic_server_tool_when_enabled() {
        let mut config = config();
        config.enable_web_search_tool = true;
        config.web_search_max_uses = Some(1);
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "input": "hi",
            "tools": [{
                "type": "web_search",
                "external_web_access": true,
                "filters": {"allowed_domains": ["openai.com", "docs.rs"]},
                "user_location": {
                    "type": "approximate",
                    "city": "Taipei",
                    "country": "TW",
                    "timezone": "Asia/Taipei"
                }
            }]
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(converted.request.tools[0]["type"], "web_search_20250305");
        assert_eq!(converted.request.tools[0]["name"], "web_search");
        assert_eq!(converted.request.tools[0]["max_uses"], 1);
        assert_eq!(
            converted.request.tools[0]["allowed_domains"],
            json!(["openai.com", "docs.rs"])
        );
        assert_eq!(
            converted.request.tools[0]["user_location"]["timezone"],
            "Asia/Taipei"
        );
        assert!(converted.request.tools[0].get("input_schema").is_none());
    }

    #[test]
    fn rejects_codex_web_search_modes_without_anthropic_equivalents() {
        let mut config = config();
        config.enable_web_search_tool = true;
        for unsupported in [
            json!({"external_web_access": false}),
            json!({"external_web_access": true, "indexed_web_access": true}),
            json!({"external_web_access": true, "search_context_size": "high"}),
            json!({"external_web_access": true, "search_content_types": ["text", "image"]}),
        ] {
            let mut tool = unsupported;
            tool["type"] = json!("web_search");
            let error = responses_to_anthropic(
                &json!({
                    "model": "Claude Sonnet 5",
                    "stream": true,
                    "input": "hi",
                    "tools": [tool]
                }),
                &config,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("cannot preserve")
                    || error.to_string().contains("no equivalent")
            );
        }
    }

    #[test]
    fn exclusive_web_search_strips_client_tools() {
        let mut config = config();
        config.enable_web_search_tool = true;
        config.web_search_exclusive = true;
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "input": "hi",
            "tools": [
                {"type":"function","name":"exec_command","parameters":{"type":"object"}},
                {"type":"namespace","name":"mcp","tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]},
                {"type":"custom","name":"apply_patch"},
                {"type":"tool_search","execution":"client","parameters":{"type":"object"}},
                {"type":"web_search","external_web_access":true}
            ]
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(converted.request.tools.len(), 1);
        assert_eq!(converted.request.tools[0]["type"], "web_search_20250305");
    }

    #[test]
    fn rejects_unknown_tool_type() {
        let body = json!({
            "model": "m",
            "stream": true,
            "input": "hi",
            "tools": [{"type":"computer_use_preview"}]
        });
        let error = responses_to_anthropic(&body, &config()).unwrap_err();
        assert!(error.to_string().contains("unsupported tool type"));
    }

    #[test]
    fn omits_legacy_openai_hosted_image_generation_tool() {
        let body = json!({
            "model": "m",
            "stream": true,
            "parallel_tool_calls": true,
            "tool_choice": "auto",
            "input": "hi",
            "tools": [{"type":"image_generation","output_format":"png"}]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert!(converted.request.tools.is_empty());
        assert!(converted.request.tool_choice.is_none());
    }

    #[test]
    fn rejects_tool_names_that_collide_after_flattening() {
        let body = json!({
            "model": "m",
            "stream": true,
            "input": "hi",
            "tools": [
                {"type":"function","name":"mcp__read","parameters":{"type":"object"}},
                {"type":"namespace","name":"mcp","tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]}
            ]
        });
        let error = responses_to_anthropic(&body, &config()).unwrap_err();
        assert!(error.to_string().contains("tool names collide"));
    }

    #[test]
    fn disables_parallel_anthropic_tool_use_when_codex_requests_serial_calls() {
        let body = json!({
            "model": "m",
            "stream": true,
            "parallel_tool_calls": false,
            "tool_choice": "auto",
            "input": "hi",
            "tools": [{"type":"function","name":"exec_command","parameters":{"type":"object"}}]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(
            converted.request.tool_choice.as_ref().unwrap()["type"],
            "auto"
        );
        assert_eq!(
            converted.request.tool_choice.as_ref().unwrap()["disable_parallel_tool_use"],
            true
        );
    }

    #[test]
    fn web_search_can_omit_large_codex_system_instructions() {
        let mut config = config();
        config.enable_web_search_tool = true;
        config.web_search_omit_system_instructions = true;
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "instructions": "AGENTS and repository instructions",
            "input": "Search OpenAI",
            "tools": [{"type":"web_search","external_web_access":true}]
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        let system = converted.request.system.unwrap();
        assert_eq!(system.len(), 1);
        assert_eq!(
            system[0],
            ContentBlock::Text {
                text: WEB_SEARCH_QUERY_INSTRUCTION.to_owned()
            }
        );
    }

    #[test]
    fn web_search_can_keep_only_latest_user_message() {
        let mut config = config();
        config.enable_web_search_tool = true;
        config.web_search_latest_user_only = true;
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"AGENTS context"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"Search OpenAI homepage"}]}
            ],
            "tools": [{"type":"web_search","external_web_access":true}]
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(converted.request.messages.len(), 1);
        assert_eq!(
            converted.request.messages[0].content,
            vec![ContentBlock::Text {
                text: "Search OpenAI homepage".to_owned()
            }]
        );
    }

    #[test]
    fn codex_function_style_web_search_becomes_server_tool() {
        let mut config = config();
        config.enable_web_search_tool = true;
        config.web_search_max_uses = Some(1);
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "input": "hi",
            "tools": [{
                "type": "function",
                "name": "web_search",
                "description": "Search the web",
                "parameters": {"type": "object", "properties": {"query": {"type": "string"}}}
            }]
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(converted.request.tools[0]["type"], "web_search_20250305");
        assert_eq!(converted.request.tools[0]["name"], "web_search");
        assert!(converted.request.tools[0].get("input_schema").is_none());
        assert_eq!(
            converted.tool_names.to_codex_name("web_search"),
            "web_search"
        );
    }
}
