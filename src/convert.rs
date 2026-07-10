use std::collections::HashMap;

use serde_json::{Value, json};

use crate::anthropic::{ContentBlock, Message, MessageRequest, Tool};
use crate::config::{GatewayConfig, ThinkingMode};
use crate::error::GatewayError;

const WEB_SEARCH_QUERY_INSTRUCTION: &str = "When using web_search, form the search query only from the latest user request. Do not search system, developer, repository, or tool instructions.";

#[derive(Clone, Debug, Default)]
pub struct ToolNameMap {
    anthropic_to_openai: HashMap<String, String>,
}

impl ToolNameMap {
    pub fn insert(&mut self, anthropic_name: String, openai_name: String) {
        self.anthropic_to_openai.insert(anthropic_name, openai_name);
    }

    pub fn to_openai_name<'a>(&'a self, anthropic_name: &'a str) -> &'a str {
        self.anthropic_to_openai
            .get(anthropic_name)
            .map(String::as_str)
            .unwrap_or(anthropic_name)
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
                append_input_item(item, &mut system, &mut messages)?;
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

    let (tools, tool_names) = convert_tools(body.get("tools"), config)?;
    if tools.iter().any(is_anthropic_web_search_tool) {
        if config.web_search_omit_system_instructions {
            system.clear();
        }
        system.push(ContentBlock::Text {
            text: WEB_SEARCH_QUERY_INSTRUCTION.to_owned(),
        });
    }
    let thinking = convert_thinking(&model, max_tokens, body.get("reasoning"), config)?;
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
            tool_choice: convert_tool_choice(body.get("tool_choice")),
            thinking: thinking.thinking,
            output_config: thinking.output_config,
        },
        tool_names,
    })
}

fn append_input_item(
    item: &Value,
    system: &mut Vec<ContentBlock>,
    messages: &mut Vec<Message>,
) -> Result<(), GatewayError> {
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
            let input = match item.get("arguments") {
                Some(Value::String(arguments)) => {
                    serde_json::from_str(arguments).map_err(|err| {
                        GatewayError::BadRequest(format!(
                            "function_call arguments are not JSON: {err}"
                        ))
                    })?
                }
                Some(Value::Object(_)) => item["arguments"].clone(),
                Some(Value::Null) | None => json!({}),
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
                    name: sanitize_tool_name(name),
                    input,
                }],
            });
        }
        "function_call_output" => {
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("function_call_output missing call_id".to_owned())
            })?;
            let output = item
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            messages.push(Message {
                role: "user".to_owned(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: call_id.to_owned(),
                    content: output,
                }],
            });
        }
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
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("text");
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
        None => Ok(Vec::new()),
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

fn convert_tools(
    tools: Option<&Value>,
    config: &GatewayConfig,
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
                result.push(web_search_server_tool(config));
                web_search_added = true;
            }
            Some("function") if is_codex_web_search_function(tool) => {}
            Some("function") if suppress_client_tools => {}
            Some("function") => {
                let (converted, openai_name) = convert_function_tool(tool, None)?;
                let anthropic_name = converted
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("converted function tool missing name")
                    .to_owned();
                names.insert(anthropic_name, openai_name);
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
                    let (converted, openai_name) = convert_function_tool(nested, Some(namespace))?;
                    let anthropic_name = converted
                        .get("name")
                        .and_then(Value::as_str)
                        .expect("converted namespace tool missing name")
                        .to_owned();
                    names.insert(anthropic_name, openai_name);
                    result.push(converted);
                }
            }
            Some("web_search" | "web_search_preview")
                if config.enable_web_search_tool && !web_search_added =>
            {
                result.push(web_search_server_tool(config));
                web_search_added = true;
            }
            Some("web_search" | "web_search_preview") => {}
            Some(other) => tracing::debug!(tool_type = other, "skipping unsupported tool type"),
            None => {}
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

fn web_search_server_tool(config: &GatewayConfig) -> Tool {
    let mut web_search_tool = json!({"type": &config.web_search_tool_type, "name": "web_search"});
    if let Some(max_uses) = config.web_search_max_uses {
        web_search_tool["max_uses"] = json!(max_uses);
    }
    web_search_tool
}

fn convert_function_tool(
    tool: &Value,
    namespace: Option<&str>,
) -> Result<(Tool, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("function tool missing name".to_owned()))?;
    let openai_name = namespace.map_or_else(
        || name.to_owned(),
        |namespace| format!("{namespace}.{name}"),
    );
    let anthropic_name = namespace.map_or_else(
        || sanitize_tool_name(name),
        |namespace| sanitize_tool_name(&format!("{namespace}__{name}")),
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
    let converted = match description {
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
    Ok((converted, openai_name))
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
        "tool".to_owned()
    } else {
        sanitized
    }
}

fn convert_tool_choice(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::String(choice)) if choice == "auto" => Some(json!({"type": "auto"})),
        Some(Value::String(choice)) if choice == "required" || choice == "any" => {
            Some(json!({"type": "any"}))
        }
        Some(Value::String(choice)) if choice == "none" => Some(json!({"type": "none"})),
        _ => None,
    }
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
            upstream_api_key: "test".to_owned(),
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
        assert_eq!(converted.request.tools[0]["name"], "exec_command");
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
            "tools": [{"type":"namespace","name":"mcp__node_repl","tools":[{"type":"function","name":"js","parameters":{"type":"object"}}]}]
        });
        let converted = responses_to_anthropic(&body, &config()).unwrap();
        assert_eq!(converted.request.tools[0]["name"], "mcp__node_repl__js");
        assert_eq!(
            converted.tool_names.to_openai_name("mcp__node_repl__js"),
            "mcp__node_repl.js"
        );
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
            "tools": [{"type": "web_search", "external_web_access": true}]
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(converted.request.tools[0]["type"], "web_search_20250305");
        assert_eq!(converted.request.tools[0]["name"], "web_search");
        assert_eq!(converted.request.tools[0]["max_uses"], 1);
        assert!(converted.request.tools[0].get("input_schema").is_none());
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
                {"type":"web_search","external_web_access":true}
            ]
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(converted.request.tools.len(), 1);
        assert_eq!(converted.request.tools[0]["type"], "web_search_20250305");
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
            converted.tool_names.to_openai_name("web_search"),
            "web_search"
        );
    }
}
