//! Chat clients use the same routing and provider execution as Responses.
use std::collections::{HashMap, HashSet};

use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::sse::SseDecoder;
use super::{CollectedResponse, ResponseStream};
use crate::error::GatewayError;

fn bad(message: impl Into<String>) -> GatewayError {
    GatewayError::BadRequest(message.into())
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, GatewayError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("missing or invalid {key}")))
}

pub(crate) fn chat_to_responses(body: &Value) -> Result<Value, GatewayError> {
    let model = string(body, "model")?;
    if body.get("n").is_some_and(|v| v != &json!(1)) {
        return Err(bad("only n=1 is supported"));
    }
    for key in [
        "stop",
        "logprobs",
        "top_logprobs",
        "logit_bias",
        "frequency_penalty",
        "presence_penalty",
        "audio",
        "modalities",
        "functions",
        "function_call",
    ] {
        if body.get(key).is_some_and(|v| !v.is_null()) {
            return Err(bad(format!(
                "{key} is not supported by the Responses gateway"
            )));
        }
    }
    if body.get("stream").is_some_and(|v| !v.is_boolean()) {
        return Err(bad("stream must be a boolean"));
    }
    let input = chat_messages(body)?;
    let mut converted = json!({"model":model,"input":input,"stream":true,"store":false});
    for key in [
        "temperature",
        "top_p",
        "parallel_tool_calls",
        "service_tier",
        "prompt_cache_key",
    ] {
        if let Some(value) = body.get(key) {
            converted[key] = value.clone();
        }
    }
    chat_options(body, &mut converted)?;
    Ok(converted)
}

fn chat_options(body: &Value, converted: &mut Value) -> Result<(), GatewayError> {
    if let Some(max) = body
        .get("max_completion_tokens")
        .or_else(|| body.get("max_tokens"))
    {
        if max.as_u64().is_none_or(|n| n == 0) {
            return Err(bad("max tokens must be a positive integer"));
        }
        converted["max_output_tokens"] = max.clone();
    }
    if let Some(effort) = body.get("reasoning_effort") {
        if !effort.is_string() {
            return Err(bad("reasoning_effort must be a string"));
        }
        converted["reasoning"] = json!({"effort":effort});
    }
    if let Some(tools) = body.get("tools") {
        let mut mapped = Vec::new();
        for tool in tools
            .as_array()
            .ok_or_else(|| bad("tools must be an array"))?
        {
            if string(tool, "type")? != "function" {
                return Err(bad("only function tools are supported"));
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(|| bad("missing tool function"))?;
            let mut function = function.clone();
            if function.get("name").and_then(Value::as_str).is_none() {
                return Err(bad("missing tool name"));
            }
            function.insert("type".to_owned(), json!("function"));
            mapped.push(Value::Object(function));
        }
        converted["tools"] = Value::Array(mapped);
    }
    if let Some(choice) = body.get("tool_choice") {
        converted["tool_choice"] = if choice.is_string() {
            choice.clone()
        } else {
            if string(choice, "type")? != "function" {
                return Err(bad("invalid tool_choice"));
            }
            json!({"type":"function","name":string(&choice["function"],"name")?})
        };
    }
    if let Some(format) = body.get("response_format") {
        converted["text"] = match string(format, "type")? {
            "text" => json!({"format":{"type":"text"}}),
            "json_schema" => {
                let mut schema = format
                    .get("json_schema")
                    .and_then(Value::as_object)
                    .cloned()
                    .ok_or_else(|| bad("missing json_schema"))?;
                schema.insert("type".to_owned(), json!("json_schema"));
                json!({"format":schema})
            }
            other => return Err(bad(format!("unsupported response_format: {other}"))),
        };
    }
    Ok(())
}

fn chat_messages(body: &Value) -> Result<Vec<Value>, GatewayError> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| bad("messages must be a nonempty array"))?;
    let mut input = Vec::new();
    for message in messages {
        append_chat_message(message, &mut input)?;
    }
    Ok(input)
}

fn append_chat_message(message: &Value, input: &mut Vec<Value>) -> Result<(), GatewayError> {
    let role = string(message, "role")?;
    if role == "tool" {
        let content = string(message, "content")?;
        input.push(json!({"type":"function_call_output","call_id":string(message,"tool_call_id")?,"output":content}));
        return Ok(());
    }
    if !matches!(role, "user" | "assistant" | "system" | "developer") {
        return Err(bad(format!("unsupported message role: {role}")));
    }
    let parts = chat_content(message, role)?;
    if !parts.is_empty() {
        input.push(json!({"type":"message","role":role,"content":parts}));
    }
    if let Some(calls) = message.get("tool_calls") {
        if role != "assistant" {
            return Err(bad("tool_calls requires an assistant message"));
        }
        for call in calls
            .as_array()
            .ok_or_else(|| bad("tool_calls must be an array"))?
        {
            if string(call, "type")? != "function" {
                return Err(bad("only function tool calls are supported"));
            }
            let function = call
                .get("function")
                .ok_or_else(|| bad("missing tool call function"))?;
            input.push(json!({"type":"function_call","call_id":string(call,"id")?,"name":string(function,"name")?,"arguments":string(function,"arguments")?}));
        }
    }
    if message
        .get("reasoning_content")
        .is_some_and(|v| !v.is_null() && v != "")
    {
        return Err(bad("reasoning_content replay is not supported"));
    }
    Ok(())
}

fn chat_content(message: &Value, role: &str) -> Result<Vec<Value>, GatewayError> {
    let mut parts = Vec::new();
    match message.get("content") {
        Some(Value::String(text)) => parts.push(
            json!({"type":if role == "assistant" {"output_text"} else {"input_text"},"text":text}),
        ),
        Some(Value::Array(content)) => {
            for part in content {
                match string(part, "type")? {
                        "text" => parts.push(json!({"type":if role == "assistant" {"output_text"} else {"input_text"},"text":string(part,"text")?})),
                        "image_url" if role == "user" => {
                            let image = part.get("image_url").ok_or_else(|| bad("missing image_url"))?;
                            let url = string(image,"url")?;
                            parts.push(json!({"type":"input_image","image_url":url,"detail":image.get("detail").cloned().unwrap_or(json!("auto"))}));
                        }
                        other => return Err(bad(format!("unsupported {role} content type: {other}"))),
                    }
            }
        }
        None | Some(Value::Null) if role == "assistant" && message.get("tool_calls").is_some() => {}
        _ => return Err(bad("message content must be text or an array")),
    }
    Ok(parts)
}

fn usage(value: &Value) -> Value {
    let prompt = value["input_tokens"].as_u64().unwrap_or(0);
    let completion = value["output_tokens"].as_u64().unwrap_or(0);
    json!({"prompt_tokens":prompt,"completion_tokens":completion,"total_tokens":prompt + completion,
        "prompt_tokens_details":{"cached_tokens":value.pointer("/input_tokens_details/cached_tokens").and_then(Value::as_u64).unwrap_or(0)},
        "completion_tokens_details":{"reasoning_tokens":value.pointer("/output_tokens_details/reasoning_tokens").and_then(Value::as_u64).unwrap_or(0)}})
}

fn finish_reason(response: &Value, used_tool: bool) -> &'static str {
    if response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
        == Some("max_output_tokens")
    {
        "length"
    } else if used_tool {
        "tool_calls"
    } else {
        "stop"
    }
}

pub(crate) fn collected_to_chat(collected: CollectedResponse, model: &str) -> Value {
    let tools: Vec<Value> = collected.output.iter().filter(|i| i["type"] == "function_call").map(|i| json!({"id":i["call_id"],"type":"function","function":{"name":i["name"],"arguments":i["arguments"]}})).collect();
    let mut message = json!({"role":"assistant","content":collected.output_text});
    if !tools.is_empty() {
        message["tool_calls"] = json!(tools);
        if collected.output_text.is_empty() {
            message["content"] = Value::Null;
        }
    }
    json!({"id":collected.response["id"],"object":"chat.completion","created":chrono::Utc::now().timestamp(),"model":model,
        "choices":[{"index":0,"message":message,"finish_reason":finish_reason(&collected.response,!tools.is_empty())}],"usage":usage(&collected.usage)})
}

fn data(payload: Value) -> Bytes {
    Bytes::from(format!("data: {payload}\n\n"))
}

struct ChatStream {
    id: String,
    model: String,
    created: i64,
    tools: HashMap<String, usize>,
    text_seen: HashSet<String>,
    args_seen: HashSet<String>,
}

impl ChatStream {
    fn chunk(&self, delta: Value, finish: Value) -> Value {
        json!({"id":self.id,"object":"chat.completion.chunk","created":self.created,"model":self.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    }

    fn item(&mut self, item: &Value, done: bool) -> Vec<Bytes> {
        let id = item["id"].as_str().unwrap_or("").to_owned();
        match item["type"].as_str() {
            Some("function_call") => {
                let mut out = Vec::new();
                let next = self.tools.len();
                if !self.tools.contains_key(&id) {
                    self.tools.insert(id.clone(), next);
                    out.push(data(self.chunk(json!({"tool_calls":[{"index":next,"id":item["call_id"],"type":"function","function":{"name":item["name"],"arguments":""}}]}),Value::Null)));
                }
                if done && self.args_seen.insert(id.clone()) {
                    out.push(data(self.chunk(json!({"tool_calls":[{"index":self.tools[&id],"function":{"arguments":item["arguments"].as_str().unwrap_or("")}}]}),Value::Null)));
                }
                out
            }
            Some("message") if done && self.text_seen.insert(id) => {
                let text = item["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["text"].as_str())
                    .collect::<String>();
                vec![data(self.chunk(json!({"content":text}), Value::Null))]
            }
            _ => Vec::new(),
        }
    }
}

pub(crate) fn responses_to_chat_stream(
    upstream: ResponseStream,
    model: String,
    include_usage: bool,
) -> ResponseStream {
    async_stream::stream! {
        let mut state = ChatStream { id:format!("chatcmpl-{}",uuid::Uuid::new_v4().simple()),model,created:chrono::Utc::now().timestamp(),tools:HashMap::new(),text_seen:HashSet::new(),args_seen:HashSet::new() };
        let mut decoder = SseDecoder::default();
        let mut upstream = upstream;
        yield Ok(data(state.chunk(json!({"role":"assistant","content":""}),Value::Null)));
        while let Some(chunk) = upstream.next().await {
            let bytes = match chunk { Ok(bytes) => bytes, Err(never) => match never {} };
            for event in decoder.push(&bytes) {
                if event.data == "[DONE]" { continue; }
                let payload: Value = match serde_json::from_str(&event.data) {
                    Ok(value) => value,
                    Err(error) => { tracing::error!(%error,"invalid Responses event for Chat client"); yield Ok(data(json!({"error":{"type":"server_error","message":"invalid upstream event"}}))); return; }
                };
                let event_type = event.event.as_deref().or_else(|| payload["type"].as_str()).unwrap_or("");
                let item_id = payload["item_id"].as_str().unwrap_or("").to_owned();
                match event_type {
                    "response.output_text.delta" => {
                        state.text_seen.insert(item_id);
                        yield Ok(data(state.chunk(json!({"content":payload["delta"]}),Value::Null)));
                    }
                    "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                        yield Ok(data(state.chunk(json!({"reasoning_content":payload["delta"]}),Value::Null)));
                    }
                    "response.output_item.added" | "response.output_item.done" => {
                        for bytes in state.item(&payload["item"],event_type.ends_with("done")) { yield Ok(bytes); }
                    }
                    "response.function_call_arguments.delta" => {
                        let Some(index) = state.tools.get(&item_id).copied() else {
                            yield Ok(data(json!({"error":{"type":"server_error","message":"tool arguments precede tool declaration"}}))); return;
                        };
                        state.args_seen.insert(item_id);
                        yield Ok(data(state.chunk(json!({"tool_calls":[{"index":index,"function":{"arguments":payload["delta"]}}]}),Value::Null)));
                    }
                    "response.completed" | "response.incomplete" => {
                        let response = &payload["response"];
                        let reason = response.pointer("/incomplete_details/reason").and_then(Value::as_str);
                        if event_type == "response.incomplete" && reason != Some("max_output_tokens") {
                            yield Ok(data(json!({"error":{"type":"server_error","message":"upstream response incomplete"}}))); return;
                        }
                        for item in response["output"].as_array().into_iter().flatten() { for bytes in state.item(item,true) { yield Ok(bytes); } }
                        yield Ok(data(state.chunk(json!({}),json!(finish_reason(response,!state.tools.is_empty())))));
                        if include_usage {
                            let mut chunk = state.chunk(json!({}),Value::Null);
                            chunk["choices"] = json!([]);
                            chunk["usage"] = usage(&response["usage"]);
                            yield Ok(data(chunk));
                        }
                        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                        return;
                    }
                    "response.failed" | "error" => {
                        yield Ok(data(json!({"error":{"type":"server_error","message":"upstream response failed"}}))); return;
                    }
                    _ => {}
                }
            }
        }
        yield Ok(data(json!({"error":{"type":"server_error","message":"upstream response ended before completion"}})));
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_tools_images_and_parameters() {
        let body = json!({"model":"model","messages":[{"role":"system","content":"policy"},{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.com/a.png"}}]},{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"read","arguments":"{}"}}]},{"role":"tool","tool_call_id":"c1","content":"ok"}],"max_tokens":12,"reasoning_effort":"high","tools":[{"type":"function","function":{"name":"read","parameters":{"type":"object"}}}]});
        let mapped = chat_to_responses(&body).unwrap();
        assert_eq!(mapped["input"][1]["content"][0]["type"], "input_image");
        assert_eq!(mapped["input"][2]["call_id"], "c1");
        assert_eq!(mapped["input"][3]["type"], "function_call_output");
        assert_eq!(mapped["max_output_tokens"], 12);
        assert_eq!(mapped["tools"][0]["name"], "read");
        let roundtrip = crate::protocol::openai_chat::responses_to_openai_chat(&mapped).unwrap();
        assert_eq!(roundtrip.request["messages"][3]["tool_call_id"], "c1");
    }

    #[tokio::test]
    async fn maps_fragmented_tool_stream_and_usage() {
        let events = concat!(
            "event: response.output_item.added\ndata: {\"item\":{\"type\":\"function_call\",\"id\":\"fc1\",\"call_id\":\"c1\",\"name\":\"read\"}}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"item_id\":\"fc1\",\"delta\":\"{}\"}\n\n",
            "event: response.output_item.done\ndata: {\"item\":{\"type\":\"function_call\",\"id\":\"fc1\",\"call_id\":\"c1\",\"name\":\"read\",\"arguments\":\"{}\"}}\n\n",
            "event: response.completed\ndata: {\"response\":{\"output\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n"
        );
        let source = futures_util::stream::iter(
            events
                .as_bytes()
                .chunks(7)
                .map(|b| Ok(Bytes::copy_from_slice(b)))
                .collect::<Vec<_>>(),
        )
        .boxed();
        let chunks: Vec<_> = responses_to_chat_stream(source, "model".to_owned(), true)
            .collect()
            .await;
        let output = chunks
            .into_iter()
            .map(|b| String::from_utf8(b.unwrap().to_vec()).unwrap())
            .collect::<String>();
        assert_eq!(output.matches("\"arguments\":\"{}\"").count(), 1);
        assert!(output.contains("\"finish_reason\":\"tool_calls\""));
        assert!(output.contains("\"total_tokens\":5"));
        assert!(output.ends_with("data: [DONE]\n\n"));
    }

    #[tokio::test]
    async fn truncated_stream_does_not_report_success() {
        let source = futures_util::stream::empty().boxed();
        let output: Vec<_> = responses_to_chat_stream(source, "model".to_owned(), false)
            .collect()
            .await;
        let output = output
            .into_iter()
            .map(|b| String::from_utf8(b.unwrap().to_vec()).unwrap())
            .collect::<String>();
        assert!(output.contains("ended before completion"));
        assert!(!output.contains("[DONE]"));
    }
}
