pub(crate) mod inbound;

use serde_json::{Value, json};

use crate::error::GatewayError;
use crate::protocol::convert::{ToolNameMap, collect_active_tools, sanitize_tool_name};
use crate::protocol::openai_chat::convert_tools;

pub const SKIP_THOUGHT_SIGNATURE_VALIDATOR: &str = "skip_thought_signature_validator";

pub(crate) fn encode_signature_in_call_id(base_id: &str, sig: &str, model: &str) -> String {
    if sig.is_empty() {
        return base_id.to_owned();
    }
    let clean_base = base_id
        .split_once("_ts_")
        .map_or(base_id, |(clean, _)| clean);
    if sig == SKIP_THOUGHT_SIGNATURE_VALIDATOR {
        return format!("{clean_base}_ts_{SKIP_THOUGHT_SIGNATURE_VALIDATOR}");
    }
    let family = if model.contains("gemini") {
        "gemini"
    } else {
        "claude"
    };
    let urlsafe: String = sig
        .chars()
        .filter(|&c| c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    format!("{clean_base}_ts_{family}_{urlsafe}")
}

pub(crate) fn extract_thought_signature(item: &Value, target_is_gemini: bool) -> String {
    if target_is_gemini {
        if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
            if call_id.starts_with("toolu_") || call_id.contains("claude") {
                return SKIP_THOUGHT_SIGNATURE_VALIDATOR.to_owned();
            }
            if let Some((_, encoded)) = call_id.split_once("_ts_")
                && encoded.starts_with("claude_")
            {
                return SKIP_THOUGHT_SIGNATURE_VALIDATOR.to_owned();
            }
        }
        if item.get("signature").is_some()
            && item.get("thoughtSignature").is_none()
            && item.get("thought_signature").is_none()
        {
            return SKIP_THOUGHT_SIGNATURE_VALIDATOR.to_owned();
        }
    }

    if let Some(sig) = item
        .get("thoughtSignature")
        .or_else(|| item.get("thought_signature"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return sig.to_owned();
    }

    if let Some(call_id) = item.get("call_id").and_then(Value::as_str)
        && let Some((_, encoded)) = call_id.split_once("_ts_")
    {
        if encoded == SKIP_THOUGHT_SIGNATURE_VALIDATOR {
            return SKIP_THOUGHT_SIGNATURE_VALIDATOR.to_owned();
        }
        let encoded_sig = if let Some(stripped) = encoded.strip_prefix("gemini_") {
            stripped
        } else if let Some(stripped) = encoded.strip_prefix("claude_") {
            if target_is_gemini {
                return SKIP_THOUGHT_SIGNATURE_VALIDATOR.to_owned();
            }
            stripped
        } else {
            encoded
        };

        if !encoded_sig.is_empty() {
            let mut b64: String = encoded_sig
                .chars()
                .map(|c| match c {
                    '-' => '+',
                    '_' => '/',
                    other => other,
                })
                .collect();
            while !b64.len().is_multiple_of(4) {
                b64.push('=');
            }
            return b64;
        }
    }

    SKIP_THOUGHT_SIGNATURE_VALIDATOR.to_owned()
}

pub(crate) fn sanitize_thought_signatures_in_gemini_payload(payload: &mut Value) {
    let target = if let Some(req) = payload.get_mut("request") {
        req
    } else {
        payload
    };
    if let Some(contents) = target.get_mut("contents").and_then(Value::as_array_mut) {
        for content in contents {
            if let Some(parts) = content.get_mut("parts").and_then(Value::as_array_mut) {
                for part in parts {
                    if part.get("functionCall").is_some() {
                        part["thoughtSignature"] = json!(SKIP_THOUGHT_SIGNATURE_VALIDATOR);
                        if let Some(obj) = part.as_object_mut() {
                            obj.remove("thought_signature");
                        }
                    } else if (part.get("thoughtSignature").is_some()
                        || part.get("thought_signature").is_some())
                        && let Some(obj) = part.as_object_mut()
                    {
                        obj.remove("thoughtSignature");
                        obj.remove("thought_signature");
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConvertedGeminiRequest {
    pub request: Value,
    pub tool_names: ToolNameMap,
}

pub fn responses_to_gemini(
    body: &Value,
    model_override: Option<&str>,
) -> Result<ConvertedGeminiRequest, GatewayError> {
    let model = match model_override {
        Some(model) => model.to_owned(),
        None => body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
            .to_owned(),
    };
    let is_gemini = model.starts_with("gemini");

    let mut system_parts = Vec::new();
    if let Some(instructions) = body
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        system_parts.push(json!({"text": instructions}));
    }

    let mut contents: Vec<Value> = Vec::new();

    let mut call_to_tool_name = std::collections::HashMap::new();
    if let Some(Value::Array(items)) = body.get("input") {
        for item in items {
            let item_type = item.get("type").and_then(Value::as_str);
            if matches!(item_type, Some("function_call" | "custom_tool_call"))
                && let Some(call_id) = item.get("call_id").and_then(Value::as_str)
                && let Some(name) = item.get("name").and_then(Value::as_str)
            {
                let upstream_name = item.get("namespace").and_then(Value::as_str).map_or_else(
                    || name.to_owned(),
                    |namespace| format!("{namespace}__{name}"),
                );
                let sanitized = sanitize_tool_name(&upstream_name);
                let clean_call_id = call_id
                    .split_once("_ts_")
                    .map_or(call_id, |(clean, _)| clean);
                call_to_tool_name.insert(clean_call_id.to_owned(), sanitized.clone());
                call_to_tool_name.insert(call_id.to_owned(), sanitized);
            }
        }
    }

    match body.get("input") {
        Some(Value::String(text)) => {
            if !text.is_empty() {
                push_part(&mut contents, "user", json!({"text": text}));
            }
        }
        Some(Value::Array(items)) => {
            for item in items {
                append_gemini_input_item(
                    item,
                    &mut contents,
                    &mut system_parts,
                    &call_to_tool_name,
                    is_gemini,
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

    if contents.is_empty() {
        return Err(GatewayError::BadRequest(
            "request has no Gemini-compatible messages".to_owned(),
        ));
    }

    let active_tools = collect_active_tools(body)?;
    let hosted_search = extract_hosted_search(&active_tools, body, &model, is_gemini)?;
    let (tools_chat, tool_names) = convert_tools(Some(&active_tools))?;

    let mut function_declarations = Vec::new();
    for tool in tools_chat {
        if let Some(function) = tool.get("function") {
            let name = function.get("name").cloned().unwrap_or(json!(""));
            let description = function.get("description").cloned().unwrap_or(json!(""));
            let mut parameters = function.get("parameters").cloned().unwrap_or(json!({
                "type": "object",
                "properties": {}
            }));
            sanitize_gemini_schema(&mut parameters);
            function_declarations.push(json!({
                "name": name,
                "description": description,
                "parameters": parameters,
            }));
        }
    }

    let generation_config = build_generation_config(body, &model);

    let mut request = json!({
        "contents": contents
    });
    if !system_parts.is_empty() {
        request["systemInstruction"] = json!({ "parts": system_parts });
    }
    if let Some(config) = generation_config {
        request["generationConfig"] = config;
    }
    let mut tools = Vec::new();
    if hosted_search {
        tools.push(json!({"googleSearch":{}}));
    }
    let has_function_tools = !function_declarations.is_empty();
    if has_function_tools {
        tools.push(json!({"functionDeclarations":function_declarations}));
    }
    if !tools.is_empty()
        && body
            .get("tool_choice")
            .is_none_or(|choice| choice != "none")
    {
        request["tools"] = json!(tools);
        if hosted_search && has_function_tools {
            // Gemini requires tool context circulation when server and client tools mix.
            request["toolConfig"] = json!({"includeServerSideToolInvocations":true});
        }
    }

    Ok(ConvertedGeminiRequest {
        request,
        tool_names,
    })
}

fn extract_hosted_search(
    active_tools: &Value,
    body: &Value,
    model: &str,
    is_gemini: bool,
) -> Result<bool, GatewayError> {
    let mut hosted_search = false;
    for tool in active_tools.as_array().into_iter().flatten() {
        if !matches!(
            tool.get("type").and_then(Value::as_str),
            Some("web_search" | "web_search_preview")
        ) {
            continue;
        }
        if !is_gemini {
            tracing::debug!(
                "omitting unavailable hosted web_search tool for non-gemini model: {model}"
            );
            continue;
        }
        for (key, value) in tool.as_object().into_iter().flatten() {
            if key == "type" || (key == "external_web_access" && value == &Value::Bool(true)) {
                continue;
            }
            return Err(GatewayError::BadRequest(format!(
                "Google Search cannot preserve web_search option {key}"
            )));
        }
        if let Some(choice) = body.get("tool_choice")
            && choice != "auto"
            && choice != "none"
        {
            return Err(GatewayError::BadRequest(
                "Google Search cannot force a requested tool_choice".to_owned(),
            ));
        }
        hosted_search = true;
    }
    Ok(hosted_search)
}

fn build_generation_config(body: &Value, model: &str) -> Option<Value> {
    if let Some(config) = body
        .get("__mixin_gemini_generation_config")
        .filter(|v| v.is_object())
    {
        return Some(config.clone());
    }
    let mut generation_config = serde_json::Map::new();
    let mut max_tokens = body
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(65536);

    if let Some(temp) = body.get("temperature") {
        generation_config.insert("temperature".to_owned(), temp.clone());
    }
    if let Some(top_p) = body.get("top_p") {
        generation_config.insert("topP".to_owned(), top_p.clone());
    }
    if let Some(top_k) = body.get("top_k") {
        generation_config.insert("topK".to_owned(), top_k.clone());
    }

    let effort = body
        .pointer("/reasoning/effort")
        .and_then(Value::as_str)
        .or_else(|| {
            if model.contains("-high") {
                Some("high")
            } else if model.contains("-medium") {
                Some("medium")
            } else if model.contains("-low") {
                Some("low")
            } else if model.contains("thinking") {
                Some("medium")
            } else {
                None
            }
        });

    if let Some(effort) = effort {
        let is_gemini = model.starts_with("gemini");
        let budget = if is_gemini {
            match effort {
                "none" | "off" => 0,
                "low" => 2048,
                "medium" => 8192,
                "high" => 16384,
                "max" | "ultra" => 32768,
                _ => 8192,
            }
        } else {
            let claude_budget = match effort {
                "none" | "off" => 0,
                "minimal" | "low" => 1024,
                "medium" => 4096,
                "high" => 8192,
                "xhigh" | "max" | "ultra" => 16384,
                _ => 4096,
            };
            if claude_budget > 0 {
                if max_tokens <= claude_budget {
                    if max_tokens > 1024 {
                        max_tokens = claude_budget + 4096;
                        claude_budget
                    } else {
                        0
                    }
                } else {
                    claude_budget
                }
            } else {
                0
            }
        };
        if budget > 0 {
            let budget = if is_gemini {
                budget
            } else {
                budget.min(max_tokens.saturating_sub(1))
            };
            generation_config.insert(
                "thinkingConfig".to_owned(),
                json!({
                    "includeThoughts": true,
                    "thinkingBudget": budget
                }),
            );
        }
    }

    generation_config.insert("maxOutputTokens".to_owned(), json!(max_tokens));

    (!generation_config.is_empty()).then_some(Value::Object(generation_config))
}

pub(crate) fn sanitize_gemini_schema(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    const ALLOWED_KEYS: &[&str] = &[
        "type",
        "format",
        "description",
        "nullable",
        "enum",
        "properties",
        "required",
        "items",
    ];
    obj.retain(|k, _| ALLOWED_KEYS.contains(&k.as_str()));

    if let Some(types) = obj.get("type").and_then(Value::as_array) {
        let is_nullable = types.iter().any(|t| t.as_str() == Some("null"));
        let main_type = types
            .iter()
            .find_map(|t| t.as_str().filter(|&s| s != "null"))
            .unwrap_or("string");
        obj.insert("type".to_owned(), json!(main_type));
        if is_nullable {
            obj.insert("nullable".to_owned(), json!(true));
        }
    }

    if obj.get("type").and_then(Value::as_str) != Some("string") {
        obj.remove("enum");
    } else if let Some(enums) = obj.get_mut("enum").and_then(Value::as_array_mut) {
        for val in enums.iter_mut() {
            if !val.is_string() {
                let s = match &*val {
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    other => other.to_string(),
                };
                *val = Value::String(s);
            }
        }
    }

    if let Some(req) = obj.get_mut("required") {
        if let Some(arr) = req.as_array_mut() {
            arr.retain(Value::is_string);
            if arr.is_empty() {
                obj.remove("required");
            }
        } else {
            obj.remove("required");
        }
    }

    if let Some(properties) = obj.get_mut("properties").and_then(Value::as_object_mut) {
        for prop in properties.values_mut() {
            sanitize_gemini_schema(prop);
        }
    }
    if let Some(items) = obj.get_mut("items") {
        sanitize_gemini_schema(items);
    }
}

fn push_part(contents: &mut Vec<Value>, role: &str, part: Value) {
    if let Some(last) = contents.last_mut()
        && last.get("role").and_then(Value::as_str) == Some(role)
        && let Some(parts) = last.get_mut("parts").and_then(Value::as_array_mut)
    {
        parts.push(part);
        return;
    }
    contents.push(json!({
        "role": role,
        "parts": [part]
    }));
}

fn append_gemini_input_item(
    item: &Value,
    contents: &mut Vec<Value>,
    system_parts: &mut Vec<Value>,
    call_to_tool_name: &std::collections::HashMap<String, String>,
    is_gemini: bool,
) -> Result<(), GatewayError> {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| {
            (item.get("role").and_then(Value::as_str).is_some() && item.get("content").is_some())
                .then_some("message")
        })
        .ok_or_else(|| GatewayError::BadRequest("input item missing type".to_owned()))?;

    match item_type {
        "message" => {
            let role = item
                .get("role")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::BadRequest("message item missing role".to_owned()))?;
            match role {
                "developer" | "system" => match item.get("content") {
                    Some(Value::String(text)) => system_parts.push(json!({"text": text})),
                    Some(Value::Array(parts)) => {
                        for part in parts {
                            let part_type =
                                part.get("type").and_then(Value::as_str).unwrap_or("text");
                            if !matches!(part_type, "input_text" | "output_text" | "text") {
                                return Err(GatewayError::BadRequest(format!(
                                    "unsupported system content part type: {part_type}"
                                )));
                            }
                            let text =
                                part.get("text").and_then(Value::as_str).ok_or_else(|| {
                                    GatewayError::BadRequest(
                                        "system content part missing text".to_owned(),
                                    )
                                })?;
                            system_parts.push(json!({"text": text}));
                        }
                    }
                    _ => {
                        return Err(GatewayError::BadRequest(
                            "system content must be a string or array".to_owned(),
                        ));
                    }
                },
                "user" => {
                    append_gemini_content(item.get("content"), contents, "user")?;
                }
                "assistant" => {
                    append_gemini_content(item.get("content"), contents, "model")?;
                }
                other => {
                    return Err(GatewayError::BadRequest(format!(
                        "unsupported message role: {other}"
                    )));
                }
            }
        }
        "function_call" => {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::BadRequest("function_call missing name".to_owned()))?;
            let upstream_name = item.get("namespace").and_then(Value::as_str).map_or_else(
                || name.to_owned(),
                |namespace| format!("{namespace}__{name}"),
            );
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("call_tool");
            let clean_call_id = call_id
                .split_once("_ts_")
                .map_or(call_id, |(clean, _)| clean);
            let arguments = match item.get("arguments") {
                Some(Value::String(s)) => {
                    serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({}))
                }
                Some(Value::Object(obj)) => Value::Object(obj.clone()),
                _ => json!({}),
            };
            let mut part = json!({
                "functionCall": {
                    "id": clean_call_id,
                    "name": sanitize_tool_name(&upstream_name),
                    "args": arguments
                }
            });
            let sig = extract_thought_signature(item, is_gemini);
            if is_gemini || sig != SKIP_THOUGHT_SIGNATURE_VALIDATOR {
                part["thoughtSignature"] = json!(sig);
            }
            push_part(contents, "model", part);
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("tool");
            let clean_call_id = call_id
                .split_once("_ts_")
                .map_or(call_id, |(clean, _)| clean);
            let tool_name = call_to_tool_name
                .get(call_id)
                .or_else(|| call_to_tool_name.get(clean_call_id))
                .cloned()
                .unwrap_or_else(|| sanitize_tool_name(clean_call_id));
            let output_str = match item.get("output") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => serde_json::to_string(other)?,
                None => String::new(),
            };
            push_part(
                contents,
                "user",
                json!({
                    "functionResponse": {
                        "id": clean_call_id,
                        "name": tool_name,
                        "response": {
                            "output": output_str
                        }
                    }
                }),
            );
        }
        "custom_tool_call" => {
            let name = item.get("name").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("custom_tool_call missing name".to_owned())
            })?;
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("call_custom_tool");
            let clean_call_id = call_id
                .split_once("_ts_")
                .map_or(call_id, |(clean, _)| clean);
            let input = item.get("input").and_then(Value::as_str).unwrap_or("");
            let sig = extract_thought_signature(item, is_gemini);
            let mut part = json!({
                "functionCall": {
                    "id": clean_call_id,
                    "name": sanitize_tool_name(name),
                    "args": { "input": input }
                }
            });
            if is_gemini || sig != SKIP_THOUGHT_SIGNATURE_VALIDATOR {
                part["thoughtSignature"] = json!(sig);
            }
            push_part(contents, "model", part);
        }
        "agent_message" => {
            let text = crate::protocol::convert::agent_message_text(item)?;
            push_part(contents, "user", json!({"text": text}));
        }
        "compaction" => {
            let token = item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !token.is_empty() {
                system_parts.push(json!({"text": format!("[compaction summary: {token}]")}));
            }
        }
        "compaction_trigger"
        | "reasoning"
        | "web_search_call"
        | "image_generation_call"
        | "additional_tools" => {}
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unsupported input item type: {other}"
            )));
        }
    }
    Ok(())
}

fn append_gemini_content(
    content: Option<&Value>,
    contents: &mut Vec<Value>,
    role: &str,
) -> Result<(), GatewayError> {
    match content {
        Some(Value::String(text)) => {
            if !text.is_empty() {
                push_part(contents, role, json!({"text": text}));
            }
            Ok(())
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("text");
                match part_type {
                    "input_text" | "output_text" | "text" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            push_part(contents, role, json!({"text": text}));
                        }
                    }
                    "input_image" => {
                        let image_url = part
                            .get("image_url")
                            .and_then(|value| {
                                value
                                    .as_str()
                                    .or_else(|| value.get("url").and_then(Value::as_str))
                            })
                            .ok_or_else(|| {
                                GatewayError::BadRequest("input_image missing image_url".to_owned())
                            })?;
                        let data_url = image_url.strip_prefix("data:").ok_or_else(|| {
                            GatewayError::BadRequest(
                                "input_image data URL must be base64".to_owned(),
                            )
                        })?;
                        let (mime, b64) = data_url.split_once(";base64,").ok_or_else(|| {
                            GatewayError::BadRequest(
                                "input_image data URL must be base64".to_owned(),
                            )
                        })?;
                        push_part(
                            contents,
                            role,
                            json!({
                                "inlineData": {
                                    "mimeType": mime,
                                    "data": b64
                                }
                            }),
                        );
                    }
                    other => {
                        return Err(GatewayError::BadRequest(format!(
                            "unsupported content part type: {other}"
                        )));
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
