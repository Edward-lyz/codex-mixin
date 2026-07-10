use std::collections::{HashMap, HashSet};
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
    id: Option<String>,
    name: Option<String>,
    start_input_json: String,
    delta_input_json: String,
    kind: ToolBlockKind,
}

#[derive(Debug)]
enum ToolBlockKind {
    Function,
    WebSearch { output_index: usize },
}

#[derive(Debug)]
struct PendingWebSearch {
    output_index: usize,
    query: String,
}

#[derive(Debug)]
struct MapperState {
    response_id: String,
    created_at: u64,
    request: Value,
    output: Vec<Value>,
    current_text: Option<TextBlock>,
    tools: HashMap<u64, ToolBlock>,
    pending_web_searches: HashMap<String, PendingWebSearch>,
    web_search_result_indexes: HashSet<u64>,
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
            pending_web_searches: HashMap::new(),
            web_search_result_indexes: HashSet::new(),
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

    fn failed_event(&self, message: impl Into<String>) -> Bytes {
        let error = json!({"message":message.into()});
        let mut response = self.response_base("failed");
        response["error"] = error.clone();
        encode_event(
            "response.failed",
            &json!({"type":"response.failed","response":response,"error":error}),
        )
        .unwrap()
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

    fn finish_tool(&mut self, index: u64) -> Result<Vec<Bytes>, String> {
        let block = self
            .tools
            .remove(&index)
            .ok_or_else(|| format!("tool call at index {index} was not started"))?;
        let id = block
            .id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| format!("tool call at index {index} missing id"))?
            .to_owned();
        let name = block
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| format!("tool call at index {index} missing name"))?
            .to_owned();
        let arguments = if block.delta_input_json.trim().is_empty() {
            block.start_input_json.trim()
        } else {
            block.delta_input_json.trim()
        };
        if let ToolBlockKind::WebSearch { output_index } = block.kind {
            let arguments = serde_json::from_str::<Value>(arguments).map_err(|err| {
                format!("web_search call {id} arguments are not valid JSON: {err}")
            })?;
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .filter(|query| !query.trim().is_empty())
                .ok_or_else(|| {
                    format!("web_search call {id} arguments must contain a non-empty query")
                })?
                .to_owned();
            if self
                .pending_web_searches
                .insert(
                    id.clone(),
                    PendingWebSearch {
                        output_index,
                        query,
                    },
                )
                .is_some()
            {
                return Err(format!("duplicate web_search tool_use id: {id}"));
            }
            return Ok(Vec::new());
        }
        let output_index = self.output.len();
        let item_id = format!("fc_{}", Uuid::new_v4().simple());
        let codex_name = self.tool_names.to_codex_name(&name);
        let item = if self.tool_names.is_custom(&name) {
            let parsed_arguments = serde_json::from_str::<Value>(arguments).map_err(|err| {
                format!("custom tool call {id} arguments are not valid JSON: {err}")
            })?;
            let input = parsed_arguments
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("custom tool call {id} arguments must contain a string input field")
                })?;
            json!({
                "type": "custom_tool_call",
                "id": item_id,
                "call_id": id,
                "name": codex_name,
                "input": input,
                "status": "completed"
            })
        } else if let Some(execution) = self.tool_names.tool_search_execution(&name) {
            let arguments = serde_json::from_str::<Value>(arguments).map_err(|err| {
                format!("tool_search call {id} arguments are not valid JSON: {err}")
            })?;
            json!({
                "type": "tool_search_call",
                "id": item_id,
                "call_id": id,
                "status": "completed",
                "execution": execution,
                "arguments": arguments
            })
        } else {
            let mut item = json!({
                "type": "function_call",
                "id": item_id,
                "call_id": id,
                "name": codex_name,
                "arguments": if arguments.is_empty() { "{}" } else { arguments },
                "status": "completed"
            });
            if let Some(namespace) = self.tool_names.to_codex_namespace(&name) {
                item["namespace"] = json!(namespace);
            }
            item
        };
        self.output.push(item.clone());
        Ok(vec![
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
        ])
    }

    fn finish_tools(&mut self) -> Result<Vec<Bytes>, String> {
        let mut indexes = self.tools.keys().copied().collect::<Vec<_>>();
        indexes.sort_unstable();
        let mut events = Vec::new();
        for index in indexes {
            events.extend(self.finish_tool(index)?);
        }
        Ok(events)
    }

    fn start_web_search(
        &mut self,
        index: u64,
        id: Option<String>,
        name: Option<String>,
        start_input_json: String,
    ) -> Result<Vec<Bytes>, String> {
        let id = id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| format!("server tool call at index {index} missing id"))?;
        if name.as_deref() != Some("web_search") {
            return Err(format!(
                "unsupported Anthropic server tool: {}",
                name.as_deref().unwrap_or("<missing>")
            ));
        }
        if self.tools.contains_key(&index) {
            return Err(format!("duplicate tool call index: {index}"));
        }
        let output_index = self.output.len();
        let item = json!({
            "type":"web_search_call",
            "id":id,
            "status":"in_progress"
        });
        self.output.push(item.clone());
        self.tools.insert(
            index,
            ToolBlock {
                id: Some(id.to_owned()),
                name,
                start_input_json,
                delta_input_json: String::new(),
                kind: ToolBlockKind::WebSearch { output_index },
            },
        );
        Ok(vec![
            encode_event(
                "response.output_item.added",
                &json!({"type":"response.output_item.added","output_index":output_index,"item":item}),
            )
            .unwrap(),
        ])
    }

    fn finish_web_search_result(
        &mut self,
        index: u64,
        content_block: &Value,
    ) -> Result<Vec<Bytes>, String> {
        if !self.web_search_result_indexes.insert(index) {
            return Err(format!("duplicate web_search result index: {index}"));
        }
        let tool_use_id = content_block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("web_search result at index {index} missing tool_use_id"))?;
        let pending = self
            .pending_web_searches
            .remove(tool_use_id)
            .ok_or_else(|| {
                format!("web_search result references unknown tool_use_id: {tool_use_id}")
            })?;
        let content = content_block
            .get("content")
            .filter(|content| content.is_array() || content.is_object())
            .ok_or_else(|| format!("web_search result at index {index} missing content"))?;
        let failed = content.get("type").and_then(Value::as_str)
            == Some("web_search_tool_result_error")
            || content.as_array().is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("web_search_tool_result_error")
                })
            });
        let item = json!({
            "type":"web_search_call",
            "id":tool_use_id,
            "status":if failed { "failed" } else { "completed" },
            "action":{"type":"search","query":pending.query}
        });
        let slot = self.output.get_mut(pending.output_index).ok_or_else(|| {
            format!(
                "web_search result output index is missing: {}",
                pending.output_index
            )
        })?;
        *slot = item.clone();
        Ok(vec![
            encode_event(
                "response.output_item.done",
                &json!({"type":"response.output_item.done","output_index":pending.output_index,"item":item}),
            )
            .unwrap(),
        ])
    }

    fn ensure_web_searches_finished(&self) -> Result<(), String> {
        if self.pending_web_searches.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Anthropic stream ended before {} web_search result block(s)",
                self.pending_web_searches.len()
            ))
        }
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
                    yield Ok(state.failed_event(err.to_string()));
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
                match handle_anthropic_event(&mut state, &data) {
                    Ok(events) => {
                        for mapped in events {
                            yield Ok(mapped);
                        }
                    }
                    Err(err) => {
                        yield Ok(state.failed_event(err));
                        return;
                    }
                }
                if data.get("type").and_then(Value::as_str) == Some("error") {
                    return;
                }
                if data.get("type").and_then(Value::as_str) == Some("message_stop") {
                    for mapped in state.finish_text() {
                        yield Ok(mapped);
                    }
                    match state.finish_tools() {
                        Ok(events) => {
                            for mapped in events {
                                yield Ok(mapped);
                            }
                        }
                        Err(err) => {
                            yield Ok(state.failed_event(err));
                            return;
                        }
                    }
                    if let Err(err) = state.ensure_web_searches_finished() {
                        yield Ok(state.failed_event(err));
                        return;
                    }
                    yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":state.completed_response()})).unwrap());
                    return;
                }
            }
        }
        for mapped in state.finish_text() {
            yield Ok(mapped);
        }
        match state.finish_tools() {
            Ok(events) => {
                for mapped in events {
                    yield Ok(mapped);
                }
            }
            Err(err) => {
                yield Ok(state.failed_event(err));
                return;
            }
        }
        if let Err(err) = state.ensure_web_searches_finished() {
            yield Ok(state.failed_event(err));
            return;
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
                    yield Ok(state.failed_event(err.to_string()));
                    return;
                }
            }
            for event in drain_events(&mut buffer) {
                if event.data == "[DONE]" {
                    for bytes in state.finish_text() {
                        yield Ok(bytes);
                    }
                    match state.finish_tools() {
                        Ok(events) => {
                            for bytes in events {
                                yield Ok(bytes);
                            }
                        }
                        Err(err) => {
                            yield Ok(state.failed_event(err));
                            return;
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
                                .map(str::to_owned),
                            name: None,
                            start_input_json: String::new(),
                            delta_input_json: String::new(),
                            kind: ToolBlockKind::Function,
                        });
                        if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                            entry.id = Some(id.to_owned());
                        }
                        if let Some(function) = tool_call.get("function") {
                            if let Some(name) = function.get("name").and_then(Value::as_str) {
                                entry.name = Some(name.to_owned());
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
                        match state.finish_tools() {
                            Ok(events) => {
                                for bytes in events {
                                    yield Ok(bytes);
                                }
                            }
                            Err(err) => {
                                yield Ok(state.failed_event(err));
                                return;
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
        match state.finish_tools() {
            Ok(events) => {
                for bytes in events {
                    yield Ok(bytes);
                }
            }
            Err(err) => {
                yield Ok(state.failed_event(err));
                return;
            }
        }
        let completed = state.completed_response();
        yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":completed})).unwrap());
    }
}

fn handle_anthropic_event(state: &mut MapperState, data: &Value) -> Result<Vec<Bytes>, String> {
    match data.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            if let Some(usage) = data.pointer("/message/usage") {
                state.usage.input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
            }
            Ok(Vec::new())
        }
        Some("content_block_start") => {
            let index = data
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "content_block_start missing index".to_owned())?;
            match data.pointer("/content_block/type").and_then(Value::as_str) {
                Some("text") => Ok(state.start_text()),
                Some("tool_use") => {
                    let id = data
                        .pointer("/content_block/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let name = data
                        .pointer("/content_block/name")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let input_json = data
                        .pointer("/content_block/input")
                        .filter(|value| !value.is_null())
                        .map(Value::to_string)
                        .unwrap_or_default();
                    if state.tools.contains_key(&index) {
                        return Err(format!("duplicate tool call index: {index}"));
                    }
                    state.tools.insert(
                        index,
                        ToolBlock {
                            id,
                            name,
                            start_input_json: input_json,
                            delta_input_json: String::new(),
                            kind: ToolBlockKind::Function,
                        },
                    );
                    Ok(state.finish_text())
                }
                Some("server_tool_use") => {
                    let content_block = data
                        .get("content_block")
                        .ok_or_else(|| "server_tool_use missing content_block".to_owned())?;
                    let mut events = state.finish_text();
                    events.extend(
                        state.start_web_search(
                            index,
                            content_block
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            content_block
                                .get("name")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            content_block
                                .get("input")
                                .filter(|input| !input.is_null())
                                .map(Value::to_string)
                                .unwrap_or_default(),
                        )?,
                    );
                    Ok(events)
                }
                Some("web_search_tool_result") => {
                    let content_block = data
                        .get("content_block")
                        .ok_or_else(|| "web_search result missing content_block".to_owned())?;
                    let mut events = state.finish_text();
                    events.extend(state.finish_web_search_result(index, content_block)?);
                    Ok(events)
                }
                _ => Ok(Vec::new()),
            }
        }
        Some("content_block_delta") => {
            let index = data
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "content_block_delta missing index".to_owned())?;
            match data.pointer("/delta/type").and_then(Value::as_str) {
                Some("text_delta") => {
                    let delta = data
                        .pointer("/delta/text")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    Ok(state.text_delta(delta))
                }
                Some("input_json_delta") => {
                    let partial = data
                        .pointer("/delta/partial_json")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "input_json_delta missing partial_json".to_owned())?;
                    let tool = state.tools.get_mut(&index).ok_or_else(|| {
                        format!("input_json_delta references unknown tool index: {index}")
                    })?;
                    tool.delta_input_json.push_str(partial);
                    Ok(Vec::new())
                }
                Some("thinking_delta" | "signature_delta") => Ok(Vec::new()),
                _ => Ok(Vec::new()),
            }
        }
        Some("content_block_stop") => {
            let index = data
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "content_block_stop missing index".to_owned())?;
            if state.tools.contains_key(&index) {
                state.finish_tool(index)
            } else if state.web_search_result_indexes.remove(&index) {
                Ok(Vec::new())
            } else {
                Ok(state.finish_text())
            }
        }
        Some("message_delta") => {
            if data.pointer("/delta/stop_reason").and_then(Value::as_str) == Some("pause_turn") {
                return Err(
                    "Anthropic returned pause_turn; automatic server-tool continuation is unsupported"
                        .to_owned(),
                );
            }
            if let Some(output_tokens) =
                data.pointer("/usage/output_tokens").and_then(Value::as_u64)
            {
                state.usage.output_tokens = Some(output_tokens);
            }
            Ok(Vec::new())
        }
        Some("error") => Ok(vec![
            state.failed_event(
                data.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream stream error"),
            ),
        ]),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collect_events<S>(events: S) -> String
    where
        S: Stream<Item = Result<Bytes, Infallible>>,
    {
        tokio::pin!(events);
        let mut output = Vec::new();
        while let Some(chunk) = events.next().await {
            output.extend_from_slice(&chunk.unwrap());
        }
        String::from_utf8(output).unwrap()
    }

    async fn map_openai_tool_call(tool_call: Value, tool_names: ToolNameMap) -> String {
        let chunk = json!({
            "choices": [{
                "delta": {"tool_calls": [tool_call]},
                "finish_reason": "tool_calls"
            }]
        });
        let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(format!(
            "data: {chunk}\n\ndata: [DONE]\n\n"
        )))]);
        collect_events(map_openai_chat_sse(upstream, json!({}), tool_names)).await
    }

    fn tool_call(id: Option<&str>, name: Option<&str>, arguments: &str) -> Value {
        let mut tool_call = json!({"index":0,"function":{"arguments":arguments}});
        if let Some(id) = id {
            tool_call["id"] = json!(id);
        }
        if let Some(name) = name {
            tool_call["function"]["name"] = json!(name);
        }
        tool_call
    }

    #[tokio::test]
    async fn maps_valid_special_and_namespaced_tool_calls() {
        let mut custom_names = ToolNameMap::default();
        custom_names
            .insert_custom("apply_patch".to_owned(), "apply_patch".to_owned())
            .unwrap();
        let custom_body = map_openai_tool_call(
            tool_call(
                Some("call_custom"),
                Some("apply_patch"),
                r#"{"input":"*** Begin Patch"}"#,
            ),
            custom_names,
        )
        .await;
        assert!(custom_body.contains("\"type\":\"custom_tool_call\""));
        assert!(
            custom_body.contains("\"input\":\"*** Begin Patch\""),
            "{custom_body}"
        );

        let mut search_names = ToolNameMap::default();
        search_names
            .insert_tool_search(
                "tool_search".to_owned(),
                "tool_search".to_owned(),
                "client".to_owned(),
            )
            .unwrap();
        let search_body = map_openai_tool_call(
            tool_call(
                Some("call_search"),
                Some("tool_search"),
                r#"{"query":"calendar"}"#,
            ),
            search_names,
        )
        .await;
        assert!(search_body.contains("\"type\":\"tool_search_call\""));
        assert!(
            search_body.contains("\"arguments\":{\"query\":\"calendar\"}"),
            "{search_body}"
        );

        let mut namespaced_names = ToolNameMap::default();
        namespaced_names
            .insert_namespaced(
                "collaboration__spawn_agent".to_owned(),
                "collaboration".to_owned(),
                "spawn_agent".to_owned(),
            )
            .unwrap();
        let namespaced_body = map_openai_tool_call(
            tool_call(Some("call_spawn"), Some("collaboration__spawn_agent"), "{}"),
            namespaced_names,
        )
        .await;
        assert!(namespaced_body.contains("\"name\":\"spawn_agent\""));
        assert!(namespaced_body.contains("\"namespace\":\"collaboration\""));
        assert!(!namespaced_body.contains("\"name\":\"collaboration__spawn_agent\""));
    }

    #[tokio::test]
    async fn rejects_malformed_openai_tool_calls() {
        let mut custom_names = ToolNameMap::default();
        custom_names
            .insert_custom("apply_patch".to_owned(), "apply_patch".to_owned())
            .unwrap();
        let mut search_names = ToolNameMap::default();
        search_names
            .insert_tool_search(
                "tool_search".to_owned(),
                "tool_search".to_owned(),
                "client".to_owned(),
            )
            .unwrap();
        let cases = [
            (
                "custom JSON",
                tool_call(Some("call_custom"), Some("apply_patch"), "{"),
                custom_names.clone(),
            ),
            (
                "custom input type",
                tool_call(Some("call_custom"), Some("apply_patch"), r#"{"input":7}"#),
                custom_names.clone(),
            ),
            (
                "custom input field",
                tool_call(
                    Some("call_custom"),
                    Some("apply_patch"),
                    r#"{"other":"value"}"#,
                ),
                custom_names,
            ),
            (
                "tool_search JSON",
                tool_call(Some("call_search"), Some("tool_search"), "{"),
                search_names,
            ),
            (
                "id",
                tool_call(None, Some("get_weather"), "{}"),
                ToolNameMap::default(),
            ),
            (
                "name",
                tool_call(Some("call_weather"), None, "{}"),
                ToolNameMap::default(),
            ),
        ];

        for (malformed, tool_call, tool_names) in cases {
            let body = map_openai_tool_call(tool_call, tool_names).await;
            assert!(
                body.contains("event: response.failed"),
                "{malformed}: {body}"
            );
            assert!(
                !body.contains("event: response.completed"),
                "{malformed}: {body}"
            );
            assert!(!body.contains("event: response.output_item.done"));
            assert!(!body.contains("call_0"));
            assert!(!body.contains("\"name\":\"\""));
            let mut encoded = body.as_bytes().to_vec();
            let failed = drain_events(&mut encoded)
                .into_iter()
                .find(|event| event.event.as_deref() == Some("response.failed"))
                .unwrap();
            let failed: Value = serde_json::from_str(&failed.data).unwrap();
            assert!(
                failed
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .is_some(),
                "{malformed}: {body}"
            );
        }
    }

    #[tokio::test]
    async fn rejects_anthropic_tool_calls_missing_id_or_name() {
        let cases = [
            (
                "id",
                json!({"type":"tool_use","name":"get_weather","input":{}}),
            ),
            (
                "name",
                json!({"type":"tool_use","id":"call_weather","input":{}}),
            ),
        ];

        for (missing, content_block) in cases {
            let start = json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": content_block
            });
            let stop = json!({"type":"content_block_stop","index":0});
            let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(
                format!("data: {start}\n\ndata: {stop}\n\ndata: {{\"type\":\"message_stop\"}}\n\n"),
            ))]);
            let body = collect_events(map_anthropic_sse(
                upstream,
                json!({}),
                ToolNameMap::default(),
            ))
            .await;

            assert!(
                body.contains("event: response.failed"),
                "missing {missing}: {body}"
            );
            assert!(
                !body.contains("event: response.completed"),
                "missing {missing}: {body}"
            );
            assert!(
                !body.contains("\"call_id\":\"\""),
                "missing {missing}: {body}"
            );
            assert!(!body.contains("\"name\":\"\""), "missing {missing}: {body}");
        }
    }

    #[tokio::test]
    async fn maps_anthropic_server_web_search_lifecycle() {
        let events = [
            json!({"type":"message_start","message":{"usage":{"input_tokens":10}}}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_123","name":"web_search","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"weather seattle\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_123","content":[{"type":"web_search_result","title":"Seattle Weather","url":"https://example.com"}]}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_stop"}),
        ];
        let stream = events
            .into_iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(stream))]);
        let body = collect_events(map_anthropic_sse(
            upstream,
            json!({"model":"Claude Sonnet 5"}),
            ToolNameMap::default(),
        ))
        .await;
        let mut encoded = body.as_bytes().to_vec();
        let events = drain_events(&mut encoded);
        let added: Value = serde_json::from_str(
            &events
                .iter()
                .find(|event| event.event.as_deref() == Some("response.output_item.added"))
                .unwrap()
                .data,
        )
        .unwrap();
        assert_eq!(added["item"]["type"], "web_search_call");
        assert_eq!(added["item"]["status"], "in_progress");
        let done: Value = serde_json::from_str(
            &events
                .iter()
                .find(|event| event.event.as_deref() == Some("response.output_item.done"))
                .unwrap()
                .data,
        )
        .unwrap();
        assert_eq!(done["item"]["id"], "srvtoolu_123");
        assert_eq!(done["item"]["status"], "completed");
        assert_eq!(done["item"]["action"]["type"], "search");
        assert_eq!(done["item"]["action"]["query"], "weather seattle");
        let completed: Value = serde_json::from_str(
            &events
                .iter()
                .find(|event| event.event.as_deref() == Some("response.completed"))
                .unwrap()
                .data,
        )
        .unwrap();
        assert_eq!(completed["response"]["output"][0], done["item"]);
    }

    #[tokio::test]
    async fn rejects_web_search_without_result_block() {
        let events = [
            json!({"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_123","name":"web_search","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"weather seattle\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_stop"}),
        ];
        let stream = events
            .into_iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(stream))]);
        let body = collect_events(map_anthropic_sse(
            upstream,
            json!({}),
            ToolNameMap::default(),
        ))
        .await;
        assert!(body.contains("event: response.failed"), "{body}");
        assert!(!body.contains("event: response.completed"), "{body}");
    }
}
