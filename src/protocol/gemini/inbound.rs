//! Gemini Developer API ingress. Reject semantics Responses cannot preserve.
//!
//! Wire contract: https://ai.google.dev/api/generate-content
//! CLI API-key transport: google-gemini/gemini-cli packages/core/src/core/contentGenerator.ts.
//! Sampling and thinking use canonical options across protocols. Exact Gemini
//! generation options survive a Gemini upstream roundtrip. Media, server tools,
//! caching and CloudCode still require their own supported wire contracts.
use crate::error::GatewayError;
use crate::protocol::sse::SseDecoder;
use crate::protocol::{CollectedResponse, ResponseStream};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::HashSet;

fn bad(message: impl Into<String>) -> GatewayError {
    GatewayError::BadRequest(message.into())
}
fn fields(value: &Value, allowed: &[&str], context: &str) -> Result<(), GatewayError> {
    let object = value
        .as_object()
        .ok_or_else(|| bad(format!("{context} must be an object")))?;
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(bad(format!("unsupported {context} field: {key}")));
        }
    }
    Ok(())
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, GatewayError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad(format!("missing or invalid {key}")))
}

pub(crate) fn to_responses(body: &Value, model: &str) -> Result<Value, GatewayError> {
    fields(
        body,
        &[
            "contents",
            "systemInstruction",
            "tools",
            "toolConfig",
            "generationConfig",
        ],
        "request",
    )?;
    if model.is_empty() {
        return Err(bad("missing model"));
    }
    let mut input = Vec::new();
    let mut pending: Vec<(String, String)> = Vec::new();
    if let Some(system) = body.get("systemInstruction") {
        fields(system, &["role", "parts"], "systemInstruction")?;
        let parts = system
            .get("parts")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("systemInstruction.parts must be an array"))?;
        let mut content = Vec::new();
        for part in parts {
            fields(part, &["text"], "systemInstruction part")?;
            let text = part
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("systemInstruction requires text"))?;
            content.push(json!({"type":"input_text", "text":text}));
        }
        input.push(json!({"type":"message", "role":"system", "content":content}));
    }
    let contents = body
        .get("contents")
        .and_then(Value::as_array)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| bad("contents must be a nonempty array"))?;
    for (turn, content) in contents.iter().enumerate() {
        fields(content, &["role", "parts"], "content")?;
        let role = match content.get("role") {
            None => "user",
            Some(role) => role
                .as_str()
                .ok_or_else(|| bad("content role must be a string"))?,
        };
        if !matches!(role, "user" | "model") {
            return Err(bad("content role must be user or model"));
        }
        let parts = content
            .get("parts")
            .and_then(Value::as_array)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| bad("parts must be a nonempty array"))?;
        for (index, part) in parts.iter().enumerate() {
            fields(
                part,
                &[
                    "text",
                    "functionCall",
                    "functionResponse",
                    "thoughtSignature",
                ],
                "part",
            )?;
            let count = ["text", "functionCall", "functionResponse"]
                .iter()
                .filter(|key| part.get(**key).is_some())
                .count();
            if count != 1 {
                return Err(bad(
                    "part requires exactly one of text/functionCall/functionResponse",
                ));
            }
            if let Some(text) = part.get("text") {
                if part.get("thoughtSignature").is_some() {
                    return Err(bad("signed text replay is not supported"));
                }
                let text = text
                    .as_str()
                    .ok_or_else(|| bad("part.text must be a string"))?;
                input.push(json!({"type":"message", "role":if role == "model" {"assistant"} else {"user"}, "content":[{"type":if role == "model" {"output_text"} else {"input_text"}, "text":text}]}));
            } else if let Some(call) = part.get("functionCall") {
                if role != "model" {
                    return Err(bad("functionCall requires model role"));
                }
                fields(call, &["name", "args", "id"], "functionCall")?;
                let name = string(call, "name")?;
                let args = call
                    .get("args")
                    .filter(|a| a.is_object())
                    .ok_or_else(|| bad("functionCall.args must be an object"))?;
                let id = match call.get("id") {
                    Some(_) => string(call, "id")?.to_owned(),
                    None => format!("gemini_{turn}_{index}"),
                };
                if pending.iter().any(|(pending_id, _)| pending_id == &id) {
                    return Err(bad("duplicate pending functionCall id"));
                }
                let mut item = json!({"type":"function_call", "call_id":id, "name":name, "arguments":args.to_string()});
                if let Some(signature) = part.get("thoughtSignature") {
                    if !signature.is_string() {
                        return Err(bad("thoughtSignature must be a string"));
                    }
                    item["thoughtSignature"] = signature.clone();
                }
                input.push(item);
                pending.push((id, name.to_owned()));
            } else if let Some(response) = part.get("functionResponse") {
                if role != "user" || part.get("thoughtSignature").is_some() {
                    return Err(bad("functionResponse requires unsigned user role"));
                }
                fields(response, &["name", "response", "id"], "functionResponse")?;
                let name = string(response, "name")?;
                let output = response
                    .get("response")
                    .filter(|r| r.is_object())
                    .ok_or_else(|| bad("functionResponse.response must be an object"))?;
                let id = response
                    .get("id")
                    .map(|_| string(response, "id"))
                    .transpose()?;
                let matches: Vec<_> = pending
                    .iter()
                    .enumerate()
                    .filter(|(_, (call_id, call_name))| {
                        call_name == name && id.is_none_or(|id| id == call_id)
                    })
                    .map(|(i, _)| i)
                    .collect();
                if matches.len() != 1 {
                    return Err(bad(
                        "functionResponse must match one pending call; use id for parallel same-name calls",
                    ));
                }
                let (id, _) = pending.remove(matches[0]);
                input.push(json!({"type":"function_call_output", "call_id":id, "output":output.to_string()}));
            }
        }
    }
    let mut converted = json!({"model":model,"input":input,"stream":true,"store":false});
    options(body, &mut converted)?;
    Ok(converted)
}

fn generation_options(body: &Value, converted: &mut Value) -> Result<(), GatewayError> {
    if let Some(config) = body.get("generationConfig") {
        fields(
            config,
            &[
                "temperature",
                "topP",
                "topK",
                "maxOutputTokens",
                "candidateCount",
                "thinkingConfig",
            ],
            "generationConfig",
        )?;
        if config.get("candidateCount").is_some_and(|v| v != &json!(1)) {
            return Err(bad("only candidateCount=1 is supported"));
        }
        for (source, target, max) in [("temperature", "temperature", 2.0), ("topP", "top_p", 1.0)] {
            if let Some(value) = config.get(source) {
                if value.as_f64().is_none_or(|n| !(0.0..=max).contains(&n)) {
                    return Err(bad(format!("invalid {source}")));
                }
                converted[target] = value.clone();
            }
        }
        if let Some(value) = config.get("maxOutputTokens") {
            if value.as_u64().is_none_or(|n| n == 0) {
                return Err(bad("maxOutputTokens must be positive"));
            }
            converted["max_output_tokens"] = value.clone();
        }
        if let Some(value) = config.get("topK") {
            if value.as_u64().is_none_or(|n| n == 0) {
                return Err(bad("topK must be a positive integer"));
            }
            converted["top_k"] = value.clone();
        }
        thinking_options(config, converted)?;
        converted["__mixin_gemini_generation_config"] = config.clone();
    }
    Ok(())
}

fn thinking_options(config: &Value, converted: &mut Value) -> Result<(), GatewayError> {
    if let Some(thinking) = config.get("thinkingConfig") {
        fields(
            thinking,
            &["includeThoughts", "thinkingBudget", "thinkingLevel"],
            "thinkingConfig",
        )?;
        if thinking
            .get("includeThoughts")
            .is_some_and(|v| !v.is_boolean())
        {
            return Err(bad("includeThoughts must be a boolean"));
        }
        if thinking.get("thinkingBudget").is_some() && thinking.get("thinkingLevel").is_some() {
            return Err(bad(
                "thinkingBudget and thinkingLevel are mutually exclusive",
            ));
        }
        let effort = if let Some(budget) = thinking.get("thinkingBudget") {
            match budget.as_i64() {
                Some(-1) => "medium",
                Some(0) => "none",
                Some(1..=2048) => "low",
                Some(2049..=8192) => "medium",
                Some(8193..) => "high",
                _ => return Err(bad("thinkingBudget must be -1 or nonnegative")),
            }
        } else if let Some(level) = thinking.get("thinkingLevel") {
            match level.as_str() {
                Some("MINIMAL" | "minimal") => "minimal",
                Some("LOW" | "low") => "low",
                Some("MEDIUM" | "medium") => "medium",
                Some("HIGH" | "high") => "high",
                _ => return Err(bad("invalid thinkingLevel")),
            }
        } else {
            "medium"
        };
        converted["reasoning"] = json!({"effort":effort});
        if thinking.get("includeThoughts").and_then(Value::as_bool) == Some(true) {
            converted["reasoning"]["summary"] = json!("auto");
        }
    }
    Ok(())
}

fn options(body: &Value, converted: &mut Value) -> Result<(), GatewayError> {
    generation_options(body, converted)?;
    if let Some(tools) = body.get("tools") {
        let tools = tools
            .as_array()
            .ok_or_else(|| bad("tools must be an array"))?;
        let mut mapped = Vec::new();
        let mut names = HashSet::new();
        for tool in tools {
            fields(tool, &["functionDeclarations"], "tool")?;
            let declarations = tool
                .get("functionDeclarations")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("tool requires functionDeclarations"))?;
            for declaration in declarations {
                fields(
                    declaration,
                    &["name", "description", "parameters", "parametersJsonSchema"],
                    "function declaration",
                )?;
                let name = string(declaration, "name")?;
                if !names.insert(name) {
                    return Err(bad("duplicate function declaration"));
                }
                if declaration.get("parameters").is_some()
                    && declaration.get("parametersJsonSchema").is_some()
                {
                    return Err(bad(
                        "parameters and parametersJsonSchema are mutually exclusive",
                    ));
                }
                let mut schema = declaration
                    .get("parameters")
                    .or_else(|| declaration.get("parametersJsonSchema"))
                    .cloned()
                    .unwrap_or(json!({"type":"object","properties":{}}));
                normalize_schema(&mut schema)?;
                let mut function = json!({"type":"function", "name":name,"parameters":schema});
                if let Some(description) = declaration.get("description") {
                    if !description.is_string() {
                        return Err(bad("description must be a string"));
                    }
                    function["description"] = description.clone();
                }
                mapped.push(function);
            }
        }
        converted["tools"] = json!(mapped);
    }
    if let Some(config) = body.get("toolConfig") {
        fields(config, &["functionCallingConfig"], "toolConfig")?;
        let config = config
            .get("functionCallingConfig")
            .ok_or_else(|| bad("missing functionCallingConfig"))?;
        fields(
            config,
            &["mode", "allowedFunctionNames"],
            "functionCallingConfig",
        )?;
        let mode = string(config, "mode")?;
        converted["tool_choice"] = match mode {
            "AUTO" => json!("auto"),
            "NONE" => json!("none"),
            "ANY" => json!("required"),
            _ => return Err(bad("unsupported function calling mode")),
        };
        if let Some(names) = config.get("allowedFunctionNames") {
            let names = names
                .as_array()
                .ok_or_else(|| bad("allowedFunctionNames must be an array"))?;
            if mode != "ANY" || names.len() != 1 {
                return Err(bad(
                    "allowedFunctionNames requires ANY and exactly one name",
                ));
            }
            let name = names[0]
                .as_str()
                .ok_or_else(|| bad("invalid allowedFunctionNames"))?;
            if !converted["tools"]
                .as_array()
                .is_some_and(|tools| tools.iter().any(|t| t["name"] == name))
            {
                return Err(bad("allowed function is not declared"));
            }
            converted["tool_choice"] = json!({"type":"function", "name":name});
        }
    }
    Ok(())
}

// Gemini Schema uses uppercase type enums; JSON Schema uses lowercase.
fn normalize_schema(schema: &mut Value) -> Result<(), GatewayError> {
    if !schema.is_object() {
        return Err(bad("function parameters must be a schema object"));
    }
    if let Some(kind) = schema.get_mut("type") {
        let name = kind
            .as_str()
            .ok_or_else(|| bad("schema type must be a string"))?
            .to_ascii_lowercase();
        if !matches!(
            name.as_str(),
            "object" | "array" | "string" | "integer" | "number" | "boolean" | "null"
        ) {
            return Err(bad("unsupported schema type"));
        }
        *kind = json!(name);
    }
    if let Some(nullable) = schema.as_object_mut().and_then(|o| o.remove("nullable")) {
        if !nullable.is_boolean() {
            return Err(bad("schema nullable must be boolean"));
        }
        if nullable == true {
            let original = std::mem::take(schema);
            *schema = json!({"anyOf":[original,{"type":"null"}]});
        }
    }
    if let Some(properties) = schema.get_mut("properties") {
        for value in properties
            .as_object_mut()
            .ok_or_else(|| bad("schema properties must be an object"))?
            .values_mut()
        {
            normalize_schema(value)?;
        }
    }
    if let Some(items) = schema.get_mut("items") {
        normalize_schema(items)?;
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = schema.get_mut(key) {
            for branch in branches
                .as_array_mut()
                .ok_or_else(|| bad("schema alternatives must be arrays"))?
            {
                normalize_schema(branch)?;
            }
        }
    }
    Ok(())
}

fn metadata(usage: &Value) -> Value {
    let input = usage["input_tokens"].as_u64().unwrap_or(0);
    let output = usage["output_tokens"].as_u64().unwrap_or(0);
    let thoughts = usage
        .pointer("/output_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({"promptTokenCount":input,"candidatesTokenCount":output.saturating_sub(thoughts),"totalTokenCount":input.saturating_add(output),
        "cachedContentTokenCount":usage.pointer("/input_tokens_details/cached_tokens").and_then(Value::as_u64).unwrap_or(0),
        "thoughtsTokenCount":thoughts})
}
fn item_parts(item: &Value) -> Result<Vec<Value>, GatewayError> {
    match item["type"].as_str() {
        Some("function_call") => {
            let args: Value = serde_json::from_str(string(item, "arguments")?)
                .map_err(|_| bad("upstream returned invalid function arguments"))?;
            if !args.is_object() {
                return Err(bad("upstream function arguments are not an object"));
            }
            let mut part = json!({"functionCall":{"id":string(item,"call_id")?,"name":string(item,"name")?,"args":args}});
            if let Some(signature) = item.get("thoughtSignature") {
                part["thoughtSignature"] = signature.clone();
            } else if item["call_id"]
                .as_str()
                .is_some_and(|id| id.contains("_ts_"))
            {
                part["thoughtSignature"] = json!(super::extract_thought_signature(item, true));
            }
            Ok(vec![part])
        }
        Some("message") => {
            let mut parts = Vec::new();
            for content in item["content"].as_array().into_iter().flatten() {
                if content["type"] != "output_text" {
                    return Err(bad("unsupported upstream message content"));
                }
                let text = content["text"]
                    .as_str()
                    .ok_or_else(|| bad("invalid upstream text"))?;
                parts.push(json!({"text":text}));
            }
            Ok(parts)
        }
        Some("reasoning") => {
            let mut parts = Vec::new();
            for summary in item["summary"].as_array().into_iter().flatten() {
                parts.push(json!({"text":string(summary,"text")?,"thought":true}));
            }
            Ok(parts)
        }
        _ => Err(bad("unsupported upstream output item")),
    }
}
fn response(parts: Vec<Value>, model: &str) -> Value {
    json!({"candidates":[{"index":0,"content":{"role":"model","parts":parts}}],"modelVersion":model})
}
fn finish(response: &Value) -> Result<&'static str, GatewayError> {
    match response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
    {
        Some("max_output_tokens") => Ok("MAX_TOKENS"),
        Some(_) => Err(bad("upstream response incomplete")),
        None => Ok("STOP"),
    }
}
pub(crate) fn from_collected(
    collected: CollectedResponse,
    model: &str,
) -> Result<Value, GatewayError> {
    let mut parts = Vec::new();
    for item in &collected.output {
        parts.extend(item_parts(item).map_err(|error| GatewayError::Upstream(error.to_string()))?);
    }
    let mut mapped = response(parts, model);
    mapped["candidates"][0]["finishReason"] = json!(finish(&collected.response)?);
    mapped["usageMetadata"] = metadata(&collected.usage);
    Ok(mapped)
}
fn data(payload: Value) -> Bytes {
    Bytes::from(format!("data: {payload}\n\n"))
}
fn stream_error() -> Bytes {
    data(
        json!({"error":{"code":502,"status":"INTERNAL","message":"Invalid or incomplete upstream response"}}),
    )
}

pub(crate) fn to_gemini_stream(upstream: ResponseStream, model: String) -> ResponseStream {
    async_stream::stream! {
        let mut upstream = upstream;
        let mut decoder = SseDecoder::default();
        let mut text_seen = HashSet::new();
        let mut items_seen = HashSet::new();
        // Gemini functionCall.args must be an object, so only complete calls can be emitted.
        let mut calls: HashSet<String> = HashSet::new();
        while let Some(chunk) = upstream.next().await {
            let bytes = match chunk { Ok(bytes) => bytes, Err(never) => match never {} };
            for event in decoder.push(&bytes) {
                if event.data == "[DONE]" { continue; }
                let payload: Value = match serde_json::from_str(&event.data) { Ok(payload) => payload, Err(_) => { yield Ok(stream_error()); return; } };
                let kind = event.event.as_deref().or_else(|| payload["type"].as_str()).unwrap_or("");
                match kind {
                    "response.output_text.delta" | "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                        let Some(text) = payload["delta"].as_str() else { yield Ok(stream_error()); return; };
                        text_seen.insert(payload["item_id"].as_str().unwrap_or("").to_owned());
                        let mut part = json!({"text":text});
                        if kind != "response.output_text.delta" { part["thought"] = json!(true); }
                        yield Ok(data(response(vec![part], &model)));
                    }
                    "response.output_item.added" => {
                        let item = &payload["item"];
                        if item["type"] == "function_call" {
                            let Some(id) = item["id"].as_str() else { yield Ok(stream_error()); return; };
                            calls.insert(id.to_owned());
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        let id = payload["item_id"].as_str().unwrap_or("");
                        if !calls.contains(id) || !payload["delta"].is_string() { yield Ok(stream_error()); return; }
                    }
                    "response.output_item.done" => {
                        let item = &payload["item"];
                        let id = item["id"].as_str().unwrap_or("").to_owned();
                        calls.remove(&id);
                        if items_seen.insert(id.clone()) && !text_seen.contains(&id) {
                            match item_parts(item) { Ok(parts) if !parts.is_empty() => yield Ok(data(response(parts,&model))), Ok(_) => {}, Err(_) => {yield Ok(stream_error()); return;} }
                        }
                    }
                    "response.completed" | "response.incomplete" => {
                        let terminal = &payload["response"];
                        if kind == "response.incomplete" && terminal.pointer("/incomplete_details/reason").and_then(Value::as_str) != Some("max_output_tokens") { yield Ok(stream_error()); return; }
                        let reason = match finish(terminal) { Ok(reason) => reason, Err(_) => {yield Ok(stream_error()); return;} };
                        for item in terminal["output"].as_array().into_iter().flatten() {
                            let id = item["id"].as_str().unwrap_or("").to_owned();
                            calls.remove(&id);
                            if items_seen.insert(id.clone()) && !text_seen.contains(&id) {
                                match item_parts(item) { Ok(parts) if !parts.is_empty() => yield Ok(data(response(parts,&model))), Ok(_) => {}, Err(_) => {yield Ok(stream_error()); return;} }
                            }
                        }
                        if !calls.is_empty() { yield Ok(stream_error()); return; }
                        let mut mapped = response(Vec::new(), &model);
                        mapped["candidates"][0]["finishReason"] = json!(reason);
                        mapped["usageMetadata"] = metadata(&terminal["usage"]);
                        yield Ok(data(mapped));
                        return;
                    }
                    "response.failed" | "error" => { yield Ok(stream_error()); return; }
                    _ => {}
                }
            }
        }
        yield Ok(stream_error());
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_inbound_usage_and_signature() {
        let usage = metadata(
            &json!({"input_tokens":5,"output_tokens":7,"input_tokens_details":{"cached_tokens":2},"output_tokens_details":{"reasoning_tokens":3}}),
        );
        assert_eq!(usage["candidatesTokenCount"], 4);
        assert_eq!(usage["totalTokenCount"], 12);
        assert_eq!(usage["thoughtsTokenCount"], 3);
        assert_eq!(usage["cachedContentTokenCount"], 2);
        let id = super::super::encode_signature_in_call_id("c1", "c2ln", "gemini-test");
        let parts = item_parts(
            &json!({"type":"function_call","call_id":id,"name":"read","arguments":"{}"}),
        )
        .unwrap();
        assert_eq!(parts[0]["thoughtSignature"], "c2ln");
    }

    #[test]
    fn gemini_inbound_tools_and_history() {
        let body = json!({"systemInstruction":{"parts":[{"text":"policy"}]},"contents":[
            {"role":"user","parts":[{"text":"read"}]},
            {"role":"model","parts":[{"functionCall":{"name":"read","args":{"path":"a"}},"thoughtSignature":"sig"}]},
            {"role":"user","parts":[{"functionResponse":{"name":"read","response":{"output":"ok"}}}]}],
            "tools":[{"functionDeclarations":[{"name":"read","parameters":{"type":"OBJECT","properties":{"path":{"type":"STRING"}}}}]}],
            "generationConfig":{"maxOutputTokens":32,"temperature":0.5,"topP":0.9,"candidateCount":1},
            "toolConfig":{"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["read"]}}});
        let converted = to_responses(&body, "vendor/model").unwrap();
        assert_eq!(converted["model"], "vendor/model");
        assert_eq!(
            converted["input"][2]["call_id"],
            converted["input"][3]["call_id"]
        );
        assert_eq!(converted["input"][2]["thoughtSignature"], "sig");
        assert_eq!(
            converted["tools"][0]["parameters"]["properties"]["path"]["type"],
            "string"
        );
        assert_eq!(converted["tool_choice"]["name"], "read");
        let chat = crate::protocol::openai_chat::responses_to_openai_chat(&converted, &|| {
            Err(crate::error::GatewayError::Other(anyhow::anyhow!("unused")))
        })
        .unwrap();
        assert_eq!(chat.request["messages"][3]["role"], "tool");
    }

    #[test]
    fn gemini_inbound_rejects_unknown_and_ambiguous_fields() {
        let base = json!({"contents":[{"parts":[{"text":"hello"}]}]});
        for (key, value) in [
            ("safetySettings", json!([])),
            ("cachedContent", json!("cache")),
            ("generationConfig", json!({"topK":0})),
            (
                "generationConfig",
                json!({"thinkingConfig":{"thinkingBudget":-2}}),
            ),
        ] {
            let mut body = base.clone();
            body[key] = value;
            assert!(to_responses(&body, "model").is_err(), "{key}");
        }
        let ambiguous = json!({"contents":[{"role":"model","parts":[{"functionCall":{"name":"read","args":{}}},{"functionCall":{"name":"read","args":{}}}]},{"parts":[{"functionResponse":{"name":"read","response":{}}}]}]});
        assert!(to_responses(&ambiguous, "model").is_err());
        assert!(
            to_responses(
                &json!({"contents":[{"parts":[{"text":"a","inlineData":{}}]}]}),
                "model"
            )
            .is_err()
        );
    }

    #[test]
    fn cli_sampling_and_thinking_survive_native_and_cross_protocol_conversion() {
        let config = json!({"temperature":0.4,"topP":0.8,"topK":64,"maxOutputTokens":4096,"thinkingConfig":{"includeThoughts":true,"thinkingBudget":512}});
        let converted = to_responses(
            &json!({"contents":[{"parts":[{"text":"hi"}]}],"generationConfig":config}),
            "gemini-test",
        )
        .unwrap();
        assert_eq!(converted["top_k"], 64);
        assert_eq!(converted["reasoning"]["effort"], "low");
        let native = crate::protocol::gemini::responses_to_gemini(&converted, None).unwrap();
        assert_eq!(native.request["generationConfig"], config);
        let chat = crate::protocol::openai_chat::responses_to_openai_chat(&converted, &|| {
            Err(bad("unused"))
        })
        .unwrap();
        assert_eq!(chat.request["top_k"], 64);
        assert_eq!(chat.request["reasoning_effort"], "low");
        assert!(
            chat.request
                .get("__mixin_gemini_generation_config")
                .is_none()
        );
        for thinking in [
            json!({"thinkingBudget":-2}),
            json!({"includeThoughts":"yes"}),
            json!({"thinkingBudget":512,"thinkingLevel":"HIGH"}),
            json!({"thinkingLevel":"unknown"}),
        ] {
            assert!(to_responses(&json!({"contents":[{"parts":[{"text":"hi"}]}],"generationConfig":{"thinkingConfig":thinking}}),"model").is_err());
        }
    }

    #[tokio::test]
    async fn gemini_inbound_fragmented_stream_tools_usage_and_finish() {
        let events = concat!(
            "event: response.output_text.delta\r\ndata: {\"item_id\":\"m1\",\"delta\":\"hello\"}\r\n\r\n",
            "event: response.output_item.done\ndata: {\"item\":{\"id\":\"m1\",\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\n",
            "event: response.output_item.added\ndata: {\"item\":{\"id\":\"f1\",\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"read\",\"arguments\":\"\"}}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"item_id\":\"f1\",\"delta\":\"{}\"}\n\n",
            "event: response.output_item.done\ndata: {\"item\":{\"id\":\"f1\",\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"read\",\"arguments\":\"{}\"}}\n\n",
            "event: response.incomplete\ndata: {\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"output\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n"
        );
        let source = futures_util::stream::iter(
            events
                .as_bytes()
                .chunks(3)
                .map(|b| Ok(Bytes::copy_from_slice(b)))
                .collect::<Vec<_>>(),
        )
        .boxed();
        let chunks: Vec<_> = to_gemini_stream(source, "model".to_owned()).collect().await;
        let output = chunks
            .into_iter()
            .map(|b| String::from_utf8(b.unwrap().to_vec()).unwrap())
            .collect::<String>();
        assert_eq!(output.matches("hello").count(), 1);
        assert_eq!(output.matches("functionCall").count(), 1);
        assert!(output.contains("MAX_TOKENS"));
        assert!(output.contains("\"totalTokenCount\":5"));
        assert!(!output.contains("[DONE]"));
    }

    #[tokio::test]
    async fn gemini_inbound_stream_yields_before_upstream_completion() {
        let source = async_stream::stream! {
            yield Ok(Bytes::from_static(b"event: response.output_text.delta\ndata: {\"item_id\":\"m1\",\"delta\":\"early\"}\n\n"));
            futures_util::future::pending::<()>().await;
        }.boxed();
        let mut mapped = to_gemini_stream(source, "model".to_owned());
        let first = tokio::time::timeout(std::time::Duration::from_secs(1), mapped.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(std::str::from_utf8(&first).unwrap().contains("early"));
    }

    #[tokio::test]
    async fn gemini_inbound_truncated_and_invalid_stream_fail() {
        for bytes in [
            b"".as_slice(),
            b"data: invalid\n\n".as_slice(),
            b"event: response.failed\ndata: {}\n\n".as_slice(),
        ] {
            let source =
                futures_util::stream::iter(vec![Ok(Bytes::copy_from_slice(bytes))]).boxed();
            let chunks: Vec<_> = to_gemini_stream(source, "model".to_owned()).collect().await;
            let output = chunks
                .into_iter()
                .map(|b| String::from_utf8(b.unwrap().to_vec()).unwrap())
                .collect::<String>();
            assert!(output.contains("INTERNAL"));
            assert!(!output.contains("STOP"));
        }
    }
}
