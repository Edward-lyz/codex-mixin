use std::collections::HashMap;
use std::convert::Infallible;

use async_stream::stream;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::convert::ToolNameMap;
use crate::sse::{drain_events, encode_event, encode_raw_event};

#[derive(Debug)]
struct TextBlock {
    output_index: usize,
    item_id: String,
    text: String,
}

#[derive(Debug)]
struct ToolBlock {
    id: String,
    name: String,
    start_input_json: String,
    delta_input_json: String,
}

#[derive(Debug)]
struct MapperState {
    response_id: String,
    created_at: u64,
    request: Value,
    output: Vec<Value>,
    current_text: Option<TextBlock>,
    tools: HashMap<u64, ToolBlock>,
    usage: Usage,
    tool_names: ToolNameMap,
}

#[derive(Clone, Debug, Default)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl MapperState {
    fn new(request: Value, tool_names: ToolNameMap) -> Self {
        Self {
            response_id: format!("resp_{}", Uuid::new_v4().simple()),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            request,
            output: Vec::new(),
            current_text: None,
            tools: HashMap::new(),
            usage: Usage::default(),
            tool_names,
        }
    }

    fn response_base(&self, status: &str) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "error": null,
            "incomplete_details": null,
            "instructions": self.request.get("instructions").cloned().unwrap_or(Value::Null),
            "max_output_tokens": self.request.get("max_output_tokens").cloned().unwrap_or(Value::Null),
            "model": self.request.get("model").cloned().unwrap_or(Value::Null),
            "output": self.output.clone(),
            "parallel_tool_calls": self.request.get("parallel_tool_calls").cloned().unwrap_or(json!(true)),
            "previous_response_id": self.request.get("previous_response_id").cloned().unwrap_or(Value::Null),
            "reasoning": self.request.get("reasoning").cloned().unwrap_or(Value::Null),
            "store": self.request.get("store").cloned().unwrap_or(json!(false)),
            "temperature": self.request.get("temperature").cloned().unwrap_or(Value::Null),
            "text": self.request.get("text").cloned().unwrap_or_else(|| json!({"format": {"type": "text"}})),
            "tool_choice": self.request.get("tool_choice").cloned().unwrap_or(json!("auto")),
            "tools": self.request.get("tools").cloned().unwrap_or_else(|| json!([])),
            "top_p": self.request.get("top_p").cloned().unwrap_or(Value::Null),
            "truncation": self.request.get("truncation").cloned().unwrap_or(Value::Null),
            "usage": Value::Null,
            "user": self.request.get("user").cloned().unwrap_or(Value::Null),
            "metadata": self.request.get("metadata").cloned().unwrap_or_else(|| json!({})),
        })
    }

    fn completed_response(&self) -> Value {
        let input_tokens = self.usage.input_tokens.unwrap_or(0);
        let output_tokens = self.usage.output_tokens.unwrap_or(0);
        let mut response = self.response_base("completed");
        response["output"] = Value::Array(self.output.clone());
        response["usage"] = json!({
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": input_tokens + output_tokens
        });
        response
    }

    fn start_text(&mut self) -> Vec<Bytes> {
        if self.current_text.is_some() {
            return Vec::new();
        }
        let output_index = self.output.len();
        let item_id = format!("msg_{}", Uuid::new_v4().simple());
        self.current_text = Some(TextBlock {
            output_index,
            item_id: item_id.clone(),
            text: String::new(),
        });
        vec![
            encode_event(
                "response.output_item.added",
                &json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":item_id,"type":"message","status":"in_progress","role":"assistant","content":[]}}),
            )
            .unwrap(),
            encode_event(
                "response.content_part.added",
                &json!({"type":"response.content_part.added","item_id":item_id,"output_index":output_index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
            )
            .unwrap(),
        ]
    }

    fn text_delta(&mut self, delta: &str) -> Vec<Bytes> {
        let mut events = self.start_text();
        let Some(block) = self.current_text.as_mut() else {
            return events;
        };
        block.text.push_str(delta);
        events.push(
            encode_event(
                "response.output_text.delta",
                &json!({"type":"response.output_text.delta","item_id":block.item_id,"output_index":block.output_index,"content_index":0,"delta":delta}),
            )
            .unwrap(),
        );
        events
    }

    fn finish_text(&mut self) -> Vec<Bytes> {
        let Some(block) = self.current_text.take() else {
            return Vec::new();
        };
        let item = json!({
            "id": block.item_id,
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type":"output_text","text":block.text,"annotations":[]}]
        });
        self.output.push(item.clone());
        vec![
            encode_event(
                "response.output_text.done",
                &json!({"type":"response.output_text.done","item_id":block.item_id,"output_index":block.output_index,"content_index":0,"text":block.text}),
            )
            .unwrap(),
            encode_event(
                "response.content_part.done",
                &json!({"type":"response.content_part.done","item_id":block.item_id,"output_index":block.output_index,"content_index":0,"part":{"type":"output_text","text":block.text,"annotations":[]}}),
            )
            .unwrap(),
            encode_event(
                "response.output_item.done",
                &json!({"type":"response.output_item.done","output_index":block.output_index,"item":item}),
            )
            .unwrap(),
        ]
    }

    fn finish_tool(&mut self, index: u64) -> Vec<Bytes> {
        let Some(block) = self.tools.remove(&index) else {
            return Vec::new();
        };
        let output_index = self.output.len();
        let arguments = if block.delta_input_json.trim().is_empty() {
            block.start_input_json.trim()
        } else {
            block.delta_input_json.trim()
        };
        let item = json!({
            "type": "function_call",
            "id": format!("fc_{}", Uuid::new_v4().simple()),
            "call_id": block.id,
            "name": self.tool_names.to_openai_name(&block.name),
            "arguments": if arguments.is_empty() { "{}" } else { arguments },
            "status": "completed"
        });
        self.output.push(item.clone());
        vec![
            encode_event(
                "response.output_item.added",
                &json!({"type":"response.output_item.added","output_index":output_index,"item":item}),
            )
            .unwrap(),
            encode_event(
                "response.output_item.done",
                &json!({"type":"response.output_item.done","output_index":output_index,"item":item}),
            )
            .unwrap(),
        ]
    }
}

pub fn map_anthropic_sse<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    stream! {
        let mut state = MapperState::new(original_request, tool_names);
        let created = state.response_base("in_progress");
        yield Ok(encode_event("response.created", &json!({"type":"response.created","response":created})).unwrap());
        yield Ok(encode_event("response.in_progress", &json!({"type":"response.in_progress","response":state.response_base("in_progress")})).unwrap());

        let mut buffer = Vec::new();
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(bytes) => buffer.extend_from_slice(&bytes),
                Err(err) => {
                    yield Ok(encode_event("response.failed", &json!({"type":"response.failed","response":state.response_base("failed"),"error":{"message":err.to_string()}})).unwrap());
                    return;
                }
            }
            for event in drain_events(&mut buffer) {
                if event.data == "[DONE]" {
                    continue;
                }
                let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                    yield Ok(encode_raw_event("response.warning", &json!({"type":"response.warning","warning":"invalid upstream SSE JSON"}).to_string()));
                    continue;
                };
                for mapped in handle_anthropic_event(&mut state, &data) {
                    yield Ok(mapped);
                }
                if data.get("type").and_then(Value::as_str) == Some("message_stop") {
                    yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":state.completed_response()})).unwrap());
                    return;
                }
            }
        }
        for mapped in state.finish_text() {
            yield Ok(mapped);
        }
        yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":state.completed_response()})).unwrap());
    }
}

pub fn map_openai_chat_sse<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    stream! {
        let mut state = MapperState::new(original_request, tool_names);
        let created = state.response_base("in_progress");
        yield Ok(encode_event("response.created", &json!({"type":"response.created","response":created})).unwrap());
        yield Ok(encode_event("response.in_progress", &json!({"type":"response.in_progress","response":state.response_base("in_progress")})).unwrap());

        let mut buffer = Vec::new();
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(bytes) => buffer.extend_from_slice(&bytes),
                Err(err) => {
                    yield Ok(encode_event("response.failed", &json!({"type":"response.failed","response":state.response_base("failed"),"error":{"message":err.to_string()}})).unwrap());
                    return;
                }
            }
            for event in drain_events(&mut buffer) {
                if event.data == "[DONE]" {
                    for bytes in state.finish_text() {
                        yield Ok(bytes);
                    }
                    let indexes = state.tools.keys().copied().collect::<Vec<_>>();
                    for index in indexes {
                        for bytes in state.finish_tool(index) {
                            yield Ok(bytes);
                        }
                    }
                    let completed = state.completed_response();
                    yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":completed})).unwrap());
                    return;
                }
                let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                    yield Ok(encode_raw_event("response.warning", &json!({"type":"response.warning","warning":"invalid upstream SSE JSON"}).to_string()));
                    continue;
                };
                if let Some(usage) = data.get("usage") {
                    state.usage.input_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
                    state.usage.output_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
                }
                let Some(choice) = data.get("choices").and_then(Value::as_array).and_then(|choices| choices.first()) else {
                    continue;
                };
                let delta = choice.get("delta").unwrap_or(&Value::Null);
                if let Some(text) = delta.get("content").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    for bytes in state.text_delta(text) {
                        yield Ok(bytes);
                    }
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let entry = state.tools.entry(index).or_insert_with(|| ToolBlock {
                            id: tool_call
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("call_0")
                                .to_owned(),
                            name: String::new(),
                            start_input_json: String::new(),
                            delta_input_json: String::new(),
                        });
                        if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                            entry.id = id.to_owned();
                        }
                        if let Some(function) = tool_call.get("function") {
                            if let Some(name) = function.get("name").and_then(Value::as_str) {
                                entry.name = name.to_owned();
                            }
                            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                                entry.delta_input_json.push_str(arguments);
                            }
                        }
                    }
                }
                match choice.get("finish_reason").and_then(Value::as_str) {
                    Some("tool_calls") => {
                        for bytes in state.finish_text() {
                            yield Ok(bytes);
                        }
                        let indexes = state.tools.keys().copied().collect::<Vec<_>>();
                        for index in indexes {
                            for bytes in state.finish_tool(index) {
                                yield Ok(bytes);
                            }
                        }
                    }
                    Some("stop") | Some("length") | Some("content_filter") => {
                        for bytes in state.finish_text() {
                            yield Ok(bytes);
                        }
                    }
                    _ => {}
                }
            }
        }
        for bytes in state.finish_text() {
            yield Ok(bytes);
        }
        let indexes = state.tools.keys().copied().collect::<Vec<_>>();
        for index in indexes {
            for bytes in state.finish_tool(index) {
                yield Ok(bytes);
            }
        }
        let completed = state.completed_response();
        yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":completed})).unwrap());
    }
}

fn handle_anthropic_event(state: &mut MapperState, data: &Value) -> Vec<Bytes> {
    match data.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            if let Some(usage) = data.pointer("/message/usage") {
                state.usage.input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
            }
            Vec::new()
        }
        Some("content_block_start") => {
            let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
            match data.pointer("/content_block/type").and_then(Value::as_str) {
                Some("text") => state.start_text(),
                Some("tool_use") => {
                    let id = data.pointer("/content_block/id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let name = data.pointer("/content_block/name").and_then(Value::as_str).unwrap_or("").to_owned();
                    let input_json = data
                        .pointer("/content_block/input")
                        .filter(|value| !value.is_null())
                        .map(Value::to_string)
                        .unwrap_or_default();
                    state.tools.insert(
                        index,
                        ToolBlock {
                            id,
                            name,
                            start_input_json: input_json,
                            delta_input_json: String::new(),
                        },
                    );
                    state.finish_text()
                }
                _ => Vec::new(),
            }
        }
        Some("content_block_delta") => {
            let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
            match data.pointer("/delta/type").and_then(Value::as_str) {
                Some("text_delta") => {
                    let delta = data.pointer("/delta/text").and_then(Value::as_str).unwrap_or("");
                    state.text_delta(delta)
                }
                Some("input_json_delta") => {
                    let partial = data.pointer("/delta/partial_json").and_then(Value::as_str).unwrap_or("");
                    if let Some(tool) = state.tools.get_mut(&index) {
                        tool.delta_input_json.push_str(partial);
                    }
                    Vec::new()
                }
                Some("thinking_delta" | "signature_delta") => Vec::new(),
                _ => Vec::new(),
            }
        }
        Some("content_block_stop") => {
            let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
            if state.tools.contains_key(&index) {
                state.finish_tool(index)
            } else {
                state.finish_text()
            }
        }
        Some("message_delta") => {
            if let Some(output_tokens) = data.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                state.usage.output_tokens = Some(output_tokens);
            }
            Vec::new()
        }
        Some("error") => vec![encode_event("response.failed", &json!({"type":"response.failed","response":state.response_base("failed"),"error":data.get("error").cloned().unwrap_or_else(|| json!({"message":"upstream stream error"}))})).unwrap()],
        _ => Vec::new(),
    }
}
