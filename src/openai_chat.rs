use serde_json::{Value, json};

use crate::convert::{
    ToolNameMap, agent_message_text, collect_active_tools, custom_tool_description,
    sanitize_tool_name,
};
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

fn append_input_item(item: &Value, messages: &mut Vec<Value>) -> Result<(), GatewayError> {
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

fn append_chat_tool_call(messages: &mut Vec<Value>, tool_call: Value) {
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

fn convert_content(content: Option<&Value>) -> Result<Value, GatewayError> {
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
        None => Ok(json!("")),
    }
}

fn tool_output_for_openai_chat(output: Option<&Value>) -> Result<Value, GatewayError> {
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

fn convert_tools(tools: Option<&Value>) -> Result<(Vec<Value>, ToolNameMap), GatewayError> {
    let Some(Value::Array(tools)) = tools else {
        return Ok((Vec::new(), ToolNameMap::default()));
    };
    let mut result = Vec::new();
    let mut tool_names = ToolNameMap::default();
    for tool in tools {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => {
                let (converted, codex_name) = convert_function_tool(tool, None)?;
                let sanitized = converted["function"]["name"]
                    .as_str()
                    .expect("converted function tool missing name")
                    .to_owned();
                tool_names.insert(sanitized, codex_name)?;
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
                    let (converted, codex_name) = convert_function_tool(nested, Some(namespace))?;
                    let sanitized = converted["function"]["name"]
                        .as_str()
                        .expect("converted namespace tool missing name")
                        .to_owned();
                    tool_names.insert_namespaced(sanitized, namespace.to_owned(), codex_name)?;
                    result.push(converted);
                }
            }
            Some("custom") => {
                let (converted, codex_name) = convert_custom_tool(tool)?;
                let upstream_name = converted["function"]["name"]
                    .as_str()
                    .expect("converted custom tool missing name")
                    .to_owned();
                tool_names.insert_custom(upstream_name, codex_name)?;
                result.push(converted);
            }
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
                let (converted, codex_name) = convert_function_tool(&function_tool, None)?;
                let upstream_name = converted["function"]["name"]
                    .as_str()
                    .expect("converted tool_search missing name")
                    .to_owned();
                tool_names.insert_tool_search(upstream_name, codex_name, execution)?;
                result.push(converted);
            }
            Some("web_search" | "web_search_preview") => {
                return Err(GatewayError::BadRequest(
                    "unsupported tool type for OpenAI Chat Completions upstream: web_search"
                        .to_owned(),
                ));
            }
            Some("image_generation") => {
                tracing::debug!("omitting legacy OpenAI-hosted image_generation tool");
            }
            Some(other) => {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported tool type for OpenAI Chat Completions upstream: {other}"
                )));
            }
            None => {
                return Err(GatewayError::BadRequest(
                    "tool definition missing type".to_owned(),
                ));
            }
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
    let codex_name = name.to_owned();
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
    let mut converted = json!({
        "type": "function",
        "function": {
            "name": sanitized_name,
            "description": description,
            "parameters": parameters
        }
    });
    if let Some(strict) = tool.get("strict").and_then(Value::as_bool) {
        converted["function"]["strict"] = json!(strict);
    }
    Ok((converted, codex_name))
}

fn convert_custom_tool(tool: &Value) -> Result<(Value, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("custom tool missing name".to_owned()))?;
    let description = custom_tool_description(tool)?;
    Ok((
        json!({
            "type": "function",
            "function": {
                "name": sanitize_tool_name(name),
                "description": description,
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": {"input": {"type": "string"}},
                    "required": ["input"],
                    "additionalProperties": false
                }
            }
        }),
        name.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_plaintext_agent_message_for_subagents() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "input": [{
                "type": "agent_message",
                "author": "/root",
                "recipient": "/root/worker",
                "content": [{"type":"input_text","text":"Inspect the repository"}]
            }]
        }))
        .unwrap();

        assert_eq!(converted.request["messages"][0]["role"], "user");
        assert_eq!(
            converted.request["messages"][0]["content"],
            "[Agent message from /root to /root/worker]\nInspect the repository"
        );
    }

    #[test]
    fn preserves_codex_namespace_tool_name() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "input": "hi",
            "tools": [{
                "type": "namespace",
                "name": "collaboration",
                "tools": [{
                    "type": "function",
                    "name": "spawn_agent",
                    "strict": true,
                    "parameters": {"type": "object"}
                }]
            }]
        }))
        .unwrap();

        assert_eq!(
            converted.request["tools"][0]["function"]["name"],
            "collaboration__spawn_agent"
        );
        assert_eq!(
            converted
                .tool_names
                .to_codex_name("collaboration__spawn_agent"),
            "spawn_agent"
        );
        assert_eq!(
            converted
                .tool_names
                .to_codex_namespace("collaboration__spawn_agent"),
            Some("collaboration")
        );
        assert_eq!(
            converted.request["tools"][0]["function"]["parameters"],
            json!({"type": "object"})
        );
        assert_eq!(converted.request["tools"][0]["function"]["strict"], true);
    }

    #[test]
    fn preserves_namespaced_tool_loop_and_parallel_setting() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "parallel_tool_calls": true,
            "tools": [{
                "type": "namespace",
                "name": "collaboration",
                "tools": [{
                    "type": "function",
                    "name": "spawn_agent",
                    "parameters": {"type": "object"}
                }]
            }],
            "input": [
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "namespace": "collaboration",
                    "name": "spawn_agent",
                    "arguments": {"task_name": "test", "message": "run"}
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "agent started"
                }
            ]
        }))
        .unwrap();

        assert_eq!(converted.request["parallel_tool_calls"], true);
        assert_eq!(
            converted.request["messages"][0]["tool_calls"][0]["function"]["name"],
            "collaboration__spawn_agent"
        );
        assert_eq!(converted.request["messages"][1]["tool_call_id"], "call_1");
    }

    #[test]
    fn groups_parallel_tool_calls_in_one_assistant_message() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "parallel_tool_calls": true,
            "input": [
                {"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"a\"}"},
                {"type":"function_call","call_id":"call_2","name":"read_file","arguments":"{\"path\":\"b\"}"},
                {"type":"function_call_output","call_id":"call_1","output":"a"},
                {"type":"function_call_output","call_id":"call_2","output":"b"}
            ]
        }))
        .unwrap();
        assert_eq!(converted.request["messages"].as_array().unwrap().len(), 3);
        assert_eq!(
            converted.request["messages"][0]["tool_calls"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(converted.request["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(converted.request["messages"][2]["tool_call_id"], "call_2");
    }

    #[test]
    fn converts_custom_and_tool_search_tools_for_chat_completions() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
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
        }))
        .unwrap();

        assert_eq!(
            converted.request["tools"][0]["function"]["name"],
            "apply_patch"
        );
        assert!(converted.tool_names.is_custom("apply_patch"));
        assert_eq!(converted.request["tools"][0]["function"]["strict"], true);
        assert!(
            converted.request["tools"][0]["function"]["description"]
                .as_str()
                .unwrap()
                .contains("start: PATCH")
        );
        assert_eq!(
            converted.request["tools"][1]["function"]["name"],
            "tool_search"
        );
        assert_eq!(
            converted.tool_names.tool_search_execution("tool_search"),
            Some("client")
        );
    }

    #[test]
    fn exposes_deferred_tools_to_chat_completions() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "tools": [{
                "type":"tool_search",
                "execution":"client",
                "description":"Search tools",
                "parameters":{"type":"object","properties":{"query":{"type":"string"}}}
            }],
            "input": [
                {"type":"tool_search_call","call_id":"search_1","execution":"client","arguments":{"query":"calendar"}},
                {"type":"tool_search_output","call_id":"search_1","status":"completed","execution":"client","tools":[
                    {"type":"namespace","name":"mcp__calendar","tools":[{"type":"function","name":"create_event","parameters":{"type":"object"}}]}
                ]}
            ]
        }))
        .unwrap();
        assert_eq!(converted.request["tools"].as_array().unwrap().len(), 2);
        assert_eq!(
            converted.request["tools"][1]["function"]["name"],
            "mcp__calendar__create_event"
        );
        assert_eq!(
            converted
                .tool_names
                .to_codex_namespace("mcp__calendar__create_event"),
            Some("mcp__calendar")
        );
    }

    #[test]
    fn rejects_tools_chat_completions_cannot_execute() {
        for tool in [
            json!({"type":"web_search"}),
            json!({"type":"computer_use_preview"}),
        ] {
            let error = responses_to_openai_chat(&json!({
                "model": "deepseek-chat",
                "stream": true,
                "input": "hi",
                "tools": [tool]
            }))
            .unwrap_err();
            assert!(error.to_string().contains("unsupported tool type"));
        }
    }

    #[test]
    fn omits_legacy_openai_hosted_image_generation_tool() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "parallel_tool_calls": true,
            "tool_choice": "auto",
            "input": "hi",
            "tools": [{"type":"image_generation","output_format":"png"}]
        }))
        .unwrap();
        assert!(converted.request.get("tools").is_none());
        assert!(converted.request.get("tool_choice").is_none());
        assert!(converted.request.get("parallel_tool_calls").is_none());
    }

    #[test]
    fn ignores_provider_native_history_when_switching_to_custom_model() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "input": [
                {"type":"reasoning","encrypted_content":"opaque","summary":[]},
                {"type":"web_search_call","id":"ws_1","status":"completed"},
                {"type":"image_generation_call","id":"ig_1","status":"completed","result":"base64"},
                {"type":"tool_search_call","execution":"server","call_id":null,"arguments":{"paths":["crm"]}},
                {"type":"tool_search_output","execution":"server","call_id":null,"status":"completed","tools":[]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
            ]
        }))
        .unwrap();
        assert_eq!(converted.request["messages"].as_array().unwrap().len(), 1);
        assert_eq!(
            converted.request["messages"][0]["content"][0]["text"],
            "continue"
        );
    }

    #[test]
    fn preserves_multimodal_tool_outputs_for_chat_completions() {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "input": [
                {"type":"function_call","call_id":"call_1","name":"view_image","arguments":"{\"path\":\"/tmp/a.png\"}"},
                {"type":"function_call_output","call_id":"call_1","output":[
                    {"type":"input_text","text":"image loaded"},
                    {"type":"input_image","image_url":"data:image/png;base64,AAAA","detail":"original"}
                ]}
            ]
        }))
        .unwrap();
        assert_eq!(
            converted.request["messages"][1]["content"],
            json!([
                {"type":"text","text":"image loaded"},
                {"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}
            ])
        );
    }
}
