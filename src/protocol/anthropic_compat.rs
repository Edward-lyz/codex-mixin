use std::collections::HashSet;
use std::convert::Infallible;

use bytes::Bytes;
use futures_util::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};

use super::{CollectedResponse, ResponseStream};
use crate::anthropic::{ContentBlock, MessageRequest};
use crate::error::GatewayError;
use crate::protocol::sse::{SseDecoder, encode_raw_event};

pub(crate) fn message_request_to_responses(
    request: &MessageRequest,
    downstream_model: &str,
) -> Result<Value, GatewayError> {
    message_request_to_responses_inner(request, downstream_model, false)
}

pub(crate) fn native_message_request_to_responses(
    request: &MessageRequest,
    raw: &Value,
    downstream_model: &str,
) -> Result<Value, GatewayError> {
    let mut request = request.clone();
    request.tools.clear();
    request.tool_choice = None;
    let mut body = message_request_to_responses_inner(&request, downstream_model, true)?;
    let mut input = Vec::new();
    let mut single = request.clone();
    single.messages.clear();
    single.system = None;
    for (message_index, message) in request.messages.iter().enumerate() {
        let raw_content = raw.pointer(&format!("/messages/{message_index}/content"));
        for (block_index, block) in message.content.iter().enumerate() {
            single.messages = vec![crate::anthropic::Message {
                role: message.role.clone(),
                content: vec![block.clone()],
            }];
            let converted = message_request_to_responses_inner(&single, downstream_model, true)?;
            let raw_block = match raw_content {
                Some(Value::Array(blocks)) => blocks.get(block_index).cloned(),
                Some(Value::String(text)) => Some(json!({"type":"text","text":text})),
                _ => None,
            };
            for mut item in converted["input"].as_array().cloned().unwrap_or_default() {
                if let Some(raw_block) = raw_block.clone() {
                    item["__mixin_native_block"] = raw_block;
                }
                input.push(item);
            }
        }
    }
    body["input"] = Value::Array(input);
    Ok(body)
}

fn message_request_to_responses_inner(
    request: &MessageRequest,
    downstream_model: &str,
    preserve_thinking: bool,
) -> Result<Value, GatewayError> {
    let mut input = Vec::new();
    for message in &request.messages {
        let mut parts = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => parts.push(json!({
                    "type": if message.role == "assistant" { "output_text" } else { "input_text" },
                    "text": text
                })),
                ContentBlock::Image { source } => {
                    if message.role == "assistant" {
                        return Err(GatewayError::BadRequest(
                            "assistant image blocks cannot be converted to OpenAI Responses"
                                .to_owned(),
                        ));
                    }
                    let image_url = anthropic_image_url(source)?;
                    parts.push(json!({"type":"input_image","image_url":image_url}));
                }
                ContentBlock::ToolUse {
                    id,
                    name,
                    input: arguments,
                } => {
                    if message.role != "assistant" {
                        return Err(GatewayError::BadRequest(
                            "tool_use must belong to an assistant message".to_owned(),
                        ));
                    }
                    if !parts.is_empty() {
                        input.push(json!({
                            "type":"message",
                            "role":message.role,
                            "content":std::mem::take(&mut parts)
                        }));
                    }
                    input.push(json!({
                        "type":"function_call",
                        "call_id":id,
                        "name":name,
                        "arguments":arguments.to_string()
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    if message.role != "user" {
                        return Err(GatewayError::BadRequest(
                            "tool_result must belong to a user message".to_owned(),
                        ));
                    }
                    if !parts.is_empty() {
                        input.push(json!({
                            "type":"message",
                            "role":message.role,
                            "content":std::mem::take(&mut parts)
                        }));
                    }
                    input.push(json!({
                        "type":"function_call_output",
                        "call_id":tool_use_id,
                        "output":anthropic_tool_output(content)?
                    }));
                }
                // Provider-native reasoning state is opaque to a different protocol.
                // It is not user content, so omit it when the mapped backend changes.
                ContentBlock::Thinking {
                    thinking,
                    signature: Some(signature),
                } if preserve_thinking => {
                    if !parts.is_empty() {
                        input.push(json!({"type":"message","role":message.role,"content":std::mem::take(&mut parts)}));
                    }
                    input.push(json!({"type":"reasoning","summary":[],"encrypted_content":super::convert::encode_anthropic_thinking(thinking, signature)}));
                }
                ContentBlock::Thinking {
                    thinking,
                    signature: None,
                } if preserve_thinking => {
                    if !parts.is_empty() {
                        input.push(json!({"type":"message","role":message.role,"content":std::mem::take(&mut parts)}));
                    }
                    input.push(json!({"type":"reasoning","summary":[],"encrypted_content":super::convert::encode_anthropic_unsigned_thinking(thinking)}));
                }
                ContentBlock::RedactedThinking { data } if preserve_thinking => {
                    if !parts.is_empty() {
                        input.push(json!({"type":"message","role":message.role,"content":std::mem::take(&mut parts)}));
                    }
                    input.push(json!({"type":"reasoning","summary":[],"encrypted_content":super::convert::encode_anthropic_redacted_thinking(data)}));
                }
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
            }
        }
        if !parts.is_empty() {
            input.push(json!({
                "type":"message",
                "role":message.role,
                "content":parts
            }));
        }
    }

    let mut body = json!({
        "model": downstream_model,
        "stream": true,
        // Messages clients carry their own history; Codex OAuth requires this.
        "store": false,
        "max_output_tokens": request.max_tokens,
        "input": input
    });
    for (name, value) in [
        ("temperature", request.temperature.map(|value| json!(value))),
        ("top_p", request.top_p.map(|value| json!(value))),
        ("top_k", request.top_k.map(|value| json!(value))),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| (name, value)))
    {
        body[name] = value;
    }
    if let Some(system) = &request.system {
        let mut instructions = Vec::with_capacity(system.len());
        for block in system {
            match block {
                ContentBlock::Text { text } => instructions.push(text.as_str()),
                _ => {
                    return Err(GatewayError::BadRequest(
                        "non-text Anthropic system blocks cannot be converted to OpenAI Responses"
                            .to_owned(),
                    ));
                }
            }
        }
        if !instructions.is_empty() {
            body["instructions"] = Value::String(instructions.join("\n\n"));
        }
    }
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(anthropic_tool_to_responses)
                .collect::<Result<Vec<_>, GatewayError>>()?,
        );
    }
    if let Some(tool_choice) = &request.tool_choice {
        let (choice, parallel) = anthropic_tool_choice(tool_choice)?;
        body["tool_choice"] = choice;
        if let Some(parallel) = parallel {
            body["parallel_tool_calls"] = Value::Bool(parallel);
        }
    }
    if let Some(effort) = request
        .output_config
        .as_ref()
        .and_then(|config| config.get("effort"))
        .and_then(Value::as_str)
    {
        body["reasoning"] = json!({"effort":effort});
    }
    if let Some(metadata) = &request.metadata {
        body["metadata"] = metadata.clone();
    }
    if request.speed.as_deref() == Some("fast") {
        body["service_tier"] = Value::String("priority".to_owned());
    }
    Ok(body)
}

fn anthropic_image_url(source: &Value) -> Result<String, GatewayError> {
    match source.get("type").and_then(Value::as_str) {
        Some("base64") => {
            let media_type = source
                .get("media_type")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    GatewayError::BadRequest("image source missing media_type".to_owned())
                })?;
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::BadRequest("image source missing data".to_owned()))?;
            Ok(format!("data:{media_type};base64,{data}"))
        }
        Some("url") => source
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| GatewayError::BadRequest("image source missing url".to_owned())),
        Some(other) => Err(GatewayError::BadRequest(format!(
            "unsupported Anthropic image source type: {other}"
        ))),
        None => Err(GatewayError::BadRequest(
            "image source missing type".to_owned(),
        )),
    }
}

fn anthropic_tool_output(content: &Value) -> Result<Value, GatewayError> {
    match content {
        Value::String(text) => Ok(Value::String(text.clone())),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| match block.get("type").and_then(Value::as_str) {
                Some("text") => block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| json!({"type":"input_text","text":text}))
                    .ok_or_else(|| {
                        GatewayError::BadRequest("tool result text missing text".to_owned())
                    }),
                Some("image") => Ok(json!({
                    "type":"input_image",
                    "image_url":anthropic_image_url(&block["source"])?
                })),
                Some(other) => Err(GatewayError::BadRequest(format!(
                    "unsupported Anthropic tool result type: {other}"
                ))),
                None => Err(GatewayError::BadRequest(
                    "tool result content block missing type".to_owned(),
                )),
            })
            .collect::<Result<Vec<_>, GatewayError>>()
            .map(Value::Array),
        _ => Err(GatewayError::BadRequest(
            "tool result content must be a string or content array".to_owned(),
        )),
    }
}

fn anthropic_tool_to_responses(tool: &Value) -> Result<Value, GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("Anthropic tool missing name".to_owned()))?;
    let input_schema = tool.get("input_schema").ok_or_else(|| {
        GatewayError::BadRequest(format!("Anthropic tool {name} missing input_schema"))
    })?;
    let mut converted = json!({
        "type":"function",
        "name":name,
        "parameters":input_schema
    });
    if let Some(description) = tool.get("description") {
        converted["description"] = description.clone();
    }
    Ok(converted)
}

fn anthropic_tool_choice(choice: &Value) -> Result<(Value, Option<bool>), GatewayError> {
    let choice_type = choice
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("tool_choice missing type".to_owned()))?;
    let converted = match choice_type {
        "auto" => Value::String("auto".to_owned()),
        "any" => Value::String("required".to_owned()),
        "none" => Value::String("none".to_owned()),
        "tool" => {
            let name = choice.get("name").and_then(Value::as_str).ok_or_else(|| {
                GatewayError::BadRequest("tool_choice type tool missing name".to_owned())
            })?;
            json!({"type":"function","name":name})
        }
        other => {
            return Err(GatewayError::BadRequest(format!(
                "unsupported Anthropic tool_choice type: {other}"
            )));
        }
    };
    let parallel = choice
        .get("disable_parallel_tool_use")
        .and_then(Value::as_bool)
        .map(|disabled| !disabled);
    Ok((converted, parallel))
}

#[derive(Debug)]
enum ActiveBlockKind {
    Text { received_delta: bool },
    Tool { received_arguments: bool },
    Thinking { received_delta: bool },
}

#[derive(Debug)]
struct ActiveBlock {
    key: String,
    index: usize,
    kind: ActiveBlockKind,
}

struct AnthropicStreamState {
    model: String,
    message_id: String,
    started: bool,
    stopped: bool,
    next_index: usize,
    blocks: Vec<ActiveBlock>,
    completed_keys: HashSet<String>,
    input_tokens: u64,
    output_tokens: u64,
    used_tool: bool,
    native: bool,
    native_usage: Option<Value>,
}

impl AnthropicStreamState {
    fn new(model: String, native: bool) -> Self {
        Self {
            model,
            message_id: format!("msg_{}", uuid::Uuid::new_v4().simple()),
            started: false,
            stopped: false,
            next_index: 0,
            blocks: Vec::new(),
            completed_keys: HashSet::new(),
            input_tokens: 0,
            output_tokens: 0,
            used_tool: false,
            native,
            native_usage: None,
        }
    }

    fn update_response(&mut self, response: &Value) {
        if let Some(id) = response.get("id").and_then(Value::as_str) {
            self.message_id = id.to_owned();
        }
        if let Some(usage) = response.get("usage") {
            self.input_tokens = usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(self.input_tokens);
            self.output_tokens = usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(self.output_tokens);
        }
    }

    fn ensure_started(&mut self) -> Vec<Bytes> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        vec![anthropic_event(
            "message_start",
            json!({
                "type":"message_start",
                "message":{
                    "id":self.message_id,
                    "type":"message",
                    "role":"assistant",
                    "content":[],
                    "model":self.model,
                    "stop_reason":Value::Null,
                    "stop_sequence":Value::Null,
                    "usage":self.native_usage.clone().unwrap_or_else(|| json!({"input_tokens":self.input_tokens,"output_tokens":0}))
                }
            }),
        )]
    }

    fn start_text(&mut self, key: String) -> Vec<Bytes> {
        let mut events = self.ensure_started();
        if self.completed_keys.contains(&key) || self.blocks.iter().any(|block| block.key == key) {
            return events;
        }
        let index = self.next_index;
        self.next_index += 1;
        self.blocks.push(ActiveBlock {
            key,
            index,
            kind: ActiveBlockKind::Text {
                received_delta: false,
            },
        });
        events.push(anthropic_event(
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
        ));
        events
    }

    fn text_delta(&mut self, key: String, text: &str) -> Vec<Bytes> {
        let mut events = self.start_text(key.clone());
        let Some(block) = self.blocks.iter_mut().find(|block| block.key == key) else {
            return events;
        };
        if let ActiveBlockKind::Text { received_delta } = &mut block.kind {
            *received_delta = true;
            if !text.is_empty() {
                events.push(anthropic_event(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":block.index,"delta":{"type":"text_delta","text":text}}),
                ));
            }
        }
        events
    }

    fn start_tool(&mut self, key: String, id: &str, name: &str) -> Vec<Bytes> {
        let mut events = self.ensure_started();
        if self.completed_keys.contains(&key) || self.blocks.iter().any(|block| block.key == key) {
            return events;
        }
        let index = self.next_index;
        self.next_index += 1;
        self.used_tool = true;
        self.blocks.push(ActiveBlock {
            key,
            index,
            kind: ActiveBlockKind::Tool {
                received_arguments: false,
            },
        });
        events.push(anthropic_event(
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
        ));
        events
    }

    fn start_thinking(&mut self, key: String) -> Vec<Bytes> {
        let mut events = self.ensure_started();
        if self.completed_keys.contains(&key) || self.blocks.iter().any(|block| block.key == key) {
            return events;
        }
        let index = self.next_index;
        self.next_index += 1;
        self.blocks.push(ActiveBlock {
            key,
            index,
            kind: ActiveBlockKind::Thinking {
                received_delta: false,
            },
        });
        events.push(anthropic_event("content_block_start", json!({"type":"content_block_start","index":index,"content_block":{"type":"thinking","thinking":""}})));
        events
    }

    fn tool_delta(&mut self, key: String, arguments: &str) -> Vec<Bytes> {
        let Some(block) = self.blocks.iter_mut().find(|block| block.key == key) else {
            return Vec::new();
        };
        if let ActiveBlockKind::Tool { received_arguments } = &mut block.kind {
            *received_arguments = true;
            if !arguments.is_empty() {
                return vec![anthropic_event(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":block.index,"delta":{"type":"input_json_delta","partial_json":arguments}}),
                )];
            }
        }
        Vec::new()
    }

    fn finish_block(&mut self, key: &str) -> Vec<Bytes> {
        let Some(position) = self.blocks.iter().position(|block| block.key == key) else {
            return Vec::new();
        };
        let block = self.blocks.remove(position);
        self.completed_keys.insert(block.key);
        vec![anthropic_event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":block.index}),
        )]
    }

    fn finish(&mut self, response: Option<&Value>, stop_reason: Option<&str>) -> Vec<Bytes> {
        if self.stopped {
            return Vec::new();
        }
        if let Some(response) = response {
            self.update_response(response);
        }
        let mut events = self.ensure_started();
        let mut indexes = self
            .blocks
            .drain(..)
            .map(|block| block.index)
            .collect::<Vec<_>>();
        indexes.sort_unstable();
        events.extend(indexes.into_iter().map(|index| {
            anthropic_event(
                "content_block_stop",
                json!({"type":"content_block_stop","index":index}),
            )
        }));
        let native_delta = response.and_then(|response| response.pointer("/__mixin_native/delta"));
        let native_stop = native_delta
            .and_then(|delta| delta.get("stop_reason"))
            .and_then(Value::as_str)
            .filter(|reason| *reason != "tool_use" || self.used_tool);
        let stop_reason = stop_reason.or(native_stop).unwrap_or(if self.used_tool {
            "tool_use"
        } else {
            "end_turn"
        });
        events.push(anthropic_event(
            "message_delta",
            json!({
                "type":"message_delta",
                "delta":{"stop_reason":stop_reason,"stop_sequence":native_delta.and_then(|delta| delta.get("stop_sequence")).cloned().unwrap_or(Value::Null)},
                "usage":response.and_then(|response| response.pointer("/__mixin_native/usage")).cloned().unwrap_or_else(|| json!({"output_tokens":self.output_tokens}))
            }),
        ));
        events.push(anthropic_event(
            "message_stop",
            json!({"type":"message_stop"}),
        ));
        self.stopped = true;
        events
    }
}

fn anthropic_event(event: &str, payload: Value) -> Bytes {
    encode_raw_event(event, &payload.to_string())
}

pub(crate) fn responses_to_anthropic_stream(
    upstream: ResponseStream,
    downstream_model: String,
    native: bool,
) -> BoxStream<'static, Result<Bytes, Infallible>> {
    async_stream::stream! {
        let mut state = AnthropicStreamState::new(downstream_model, native);
        let mut decoder = SseDecoder::default();
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(never) => match never {},
            };
            for event in decoder.push(&bytes) {
                let Ok(payload) = serde_json::from_str::<Value>(&event.data) else {
                    yield Ok(anthropic_event(
                        "error",
                        json!({"type":"error","error":{"type":"api_error","message":"invalid internal Responses SSE JSON"}}),
                    ));
                    return;
                };
                let event_type = payload
                    .get("type")
                    .and_then(Value::as_str)
                    .or(event.event.as_deref())
                    .unwrap_or("message");
                let events = handle_responses_event(&mut state, event_type, &payload);
                for mapped in events {
                    yield Ok(mapped);
                }
                if event_type == "response.failed"
                    || (event_type == "response.incomplete" && !state.stopped)
                {
                    let message = payload
                        .pointer("/error/message")
                        .or_else(|| payload.pointer("/response/error/message"))
                        .and_then(Value::as_str)
                        .unwrap_or("upstream response failed");
                    yield Ok(anthropic_event(
                        "error",
                        json!({"type":"error","error":{"type":"api_error","message":message}}),
                    ));
                    return;
                }
                if state.stopped {
                    return;
                }
            }
        }
        yield Ok(anthropic_event(
            "error",
            json!({"type":"error","error":{"type":"api_error","message":"upstream response ended before completion"}}),
        ));
    }
    .boxed()
}

fn handle_responses_event(
    state: &mut AnthropicStreamState,
    event_type: &str,
    payload: &Value,
) -> Vec<Bytes> {
    match event_type {
        "response.created" | "response.in_progress" => {
            if let Some(response) = payload.get("response") {
                state.update_response(response);
            }
            if state.native {
                Vec::new()
            } else {
                state.ensure_started()
            }
        }
        "response.native_message_start" => {
            if !state.started {
                if let Some(id) = payload.pointer("/message/id").and_then(Value::as_str) {
                    state.message_id = id.to_owned();
                }
                state.native_usage = payload.pointer("/message/usage").cloned();
            }
            state.ensure_started()
        }
        "response.content_part.added" => {
            if payload.pointer("/part/type").and_then(Value::as_str) != Some("output_text") {
                return Vec::new();
            }
            let key = response_item_key(payload, None);
            state.start_text(key)
        }
        "response.output_text.delta" => {
            let key = response_item_key(payload, None);
            let text = payload.get("delta").and_then(Value::as_str).unwrap_or("");
            state.text_delta(key, text)
        }
        "response.reasoning_summary_text.delta" => {
            let key = response_item_key(payload, None);
            let mut events = state.start_thinking(key.clone());
            if let Some(block) = state.blocks.iter_mut().find(|block| block.key == key) {
                if let ActiveBlockKind::Thinking { received_delta } = &mut block.kind {
                    *received_delta = true;
                }
                events.push(anthropic_event("content_block_delta", json!({"type":"content_block_delta","index":block.index,"delta":{"type":"thinking_delta","thinking":payload.get("delta").and_then(Value::as_str).unwrap_or("")}})));
            }
            events
        }
        "response.output_text.done" => {
            let key = response_item_key(payload, None);
            let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
            let received_delta = state.blocks.iter().any(|block| {
                block.key == key
                    && matches!(
                        block.kind,
                        ActiveBlockKind::Text {
                            received_delta: true
                        }
                    )
            });
            let mut events = if received_delta {
                Vec::new()
            } else {
                state.text_delta(key.clone(), text)
            };
            events.extend(state.finish_block(&key));
            events
        }
        "response.output_item.added" | "response.output_item.done" => {
            let Some(item) = payload.get("item") else {
                return Vec::new();
            };
            handle_output_item(state, item, payload, event_type.ends_with("done"))
        }
        "response.function_call_arguments.delta" => {
            let key = response_item_key(payload, None);
            let delta = payload.get("delta").and_then(Value::as_str).unwrap_or("");
            state.tool_delta(key, delta)
        }
        "response.function_call_arguments.done" => {
            let key = response_item_key(payload, None);
            let received_arguments = state.blocks.iter().any(|block| {
                block.key == key
                    && matches!(
                        block.kind,
                        ActiveBlockKind::Tool {
                            received_arguments: true
                        }
                    )
            });
            let mut events = Vec::new();
            if !received_arguments {
                let arguments = payload
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                events.extend(state.tool_delta(key.clone(), arguments));
            }
            events.extend(state.finish_block(&key));
            events
        }
        "response.completed" => {
            let response = payload.get("response");
            let mut events = Vec::new();
            if let Some(output) = response
                .and_then(|response| response.get("output"))
                .and_then(Value::as_array)
            {
                for item in output {
                    events.extend(handle_output_item(state, item, item, true));
                }
            }
            events.extend(state.finish(response, None));
            events
        }
        "response.incomplete"
            if payload
                .pointer("/response/incomplete_details/reason")
                .and_then(Value::as_str)
                == Some("max_output_tokens") =>
        {
            state.finish(payload.get("response"), Some("max_tokens"))
        }
        _ => Vec::new(),
    }
}

fn handle_output_item(
    state: &mut AnthropicStreamState,
    item: &Value,
    envelope: &Value,
    done: bool,
) -> Vec<Bytes> {
    let key = response_item_key(envelope, Some(item));
    match item.get("type").and_then(Value::as_str) {
        Some("reasoning") if done => {
            let native = item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .and_then(|value| value.strip_prefix("codex-mixin:anthropic-thinking:v1:"))
                .and_then(|value| serde_json::from_str::<Value>(value).ok());
            match native
                .as_ref()
                .and_then(|block| block.get("type"))
                .and_then(Value::as_str)
            {
                Some("redacted_thinking") => {
                    if state.completed_keys.contains(&key) {
                        return Vec::new();
                    }
                    let mut events = state.ensure_started();
                    let index = state.next_index;
                    state.next_index += 1;
                    state.completed_keys.insert(key);
                    events.push(anthropic_event(
                        "content_block_start",
                        json!({"type":"content_block_start","index":index,"content_block":native}),
                    ));
                    events.push(anthropic_event(
                        "content_block_stop",
                        json!({"type":"content_block_stop","index":index}),
                    ));
                    events
                }
                _ => {
                    let mut events = state.start_thinking(key.clone());
                    if let Some(block) = state.blocks.iter().find(|block| block.key == key)
                        && matches!(
                            block.kind,
                            ActiveBlockKind::Thinking {
                                received_delta: false
                            }
                        )
                    {
                        let text = native
                            .as_ref()
                            .and_then(|block| block.get("thinking"))
                            .and_then(Value::as_str)
                            .or_else(|| item.pointer("/summary/0/text").and_then(Value::as_str))
                            .unwrap_or("");
                        if !text.is_empty() {
                            events.push(anthropic_event("content_block_delta", json!({"type":"content_block_delta","index":block.index,"delta":{"type":"thinking_delta","thinking":text}})));
                        }
                    }
                    if let Some(signature) = native
                        .as_ref()
                        .and_then(|block| block.get("signature"))
                        .and_then(Value::as_str)
                        && let Some(block) = state.blocks.iter().find(|block| block.key == key)
                    {
                        events.push(anthropic_event("content_block_delta", json!({"type":"content_block_delta","index":block.index,"delta":{"type":"signature_delta","signature":signature}})));
                    }
                    events.extend(state.finish_block(&key));
                    events
                }
            }
        }
        Some("message") if done => {
            let text = item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            let received_delta = state.blocks.iter().any(|block| {
                block.key == key
                    && matches!(
                        block.kind,
                        ActiveBlockKind::Text {
                            received_delta: true
                        }
                    )
            });
            let mut events = if received_delta {
                Vec::new()
            } else {
                state.text_delta(key.clone(), &text)
            };
            events.extend(state.finish_block(&key));
            events
        }
        Some("function_call") => {
            let id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("call_unknown");
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown_tool");
            let mut events = state.start_tool(key.clone(), id, name);
            if done {
                let received_arguments = state.blocks.iter().any(|block| {
                    block.key == key
                        && matches!(
                            block.kind,
                            ActiveBlockKind::Tool {
                                received_arguments: true
                            }
                        )
                });
                if !received_arguments {
                    events.extend(
                        state.tool_delta(
                            key.clone(),
                            item.get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}"),
                        ),
                    );
                }
                events.extend(state.finish_block(&key));
            }
            events
        }
        _ => Vec::new(),
    }
}

fn response_item_key(envelope: &Value, item: Option<&Value>) -> String {
    envelope
        .get("item_id")
        .or_else(|| item.and_then(|item| item.get("id")))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "output:{}",
                envelope
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            )
        })
}

pub(crate) fn collected_to_anthropic_message(
    collected: CollectedResponse,
    downstream_model: &str,
) -> Result<Value, GatewayError> {
    let mut content = Vec::new();
    let mut used_tool = false;
    for item in collected.output {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                if let Some(block) = item
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .and_then(|value| value.strip_prefix("codex-mixin:anthropic-thinking:v1:"))
                    .and_then(|value| serde_json::from_str::<Value>(value).ok())
                {
                    content.push(block);
                } else if let Some(text) = item.pointer("/summary/0/text").and_then(Value::as_str) {
                    content.push(json!({"type":"thinking","thinking":text}));
                }
            }
            Some("message") => {
                for part in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if part.get("type").and_then(Value::as_str) == Some("output_text") {
                        let text = part.get("text").and_then(Value::as_str).ok_or_else(|| {
                            GatewayError::Upstream(
                                "Responses output_text item missing text".to_owned(),
                            )
                        })?;
                        content.push(json!({"type":"text","text":text}));
                    }
                }
            }
            Some("function_call") => {
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        GatewayError::Upstream(
                            "Responses function_call item missing arguments".to_owned(),
                        )
                    })?;
                let input: Value = serde_json::from_str(arguments).map_err(|error| {
                    GatewayError::Upstream(format!(
                        "Responses function_call arguments are not JSON: {error}"
                    ))
                })?;
                content.push(json!({
                    "type":"tool_use",
                    "id":item.get("call_id").or_else(|| item.get("id")).and_then(Value::as_str).unwrap_or("call_unknown"),
                    "name":item.get("name").and_then(Value::as_str).unwrap_or("unknown_tool"),
                    "input":input
                }));
                used_tool = true;
            }
            _ => {}
        }
    }
    let native = collected.response.get("__mixin_native");
    let native_stop = native
        .and_then(|native| native.pointer("/delta/stop_reason"))
        .and_then(Value::as_str)
        .filter(|reason| *reason != "tool_use" || used_tool);
    let stop_reason = if used_tool {
        "tool_use"
    } else if let Some(reason) = native_stop {
        reason
    } else if collected
        .response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
        == Some("max_output_tokens")
    {
        "max_tokens"
    } else {
        "end_turn"
    };
    Ok(json!({
        "id":native.and_then(|native| native.get("id")).and_then(Value::as_str).or_else(|| collected.response.get("id").and_then(Value::as_str)).unwrap_or("msg_unknown"),
        "type":"message",
        "role":"assistant",
        "model":downstream_model,
        "content":content,
        "stop_reason":stop_reason,
        "stop_sequence":native.and_then(|native| native.pointer("/delta/stop_sequence")).cloned().unwrap_or(Value::Null),
        "usage":native.and_then(|native| native.get("usage")).cloned().unwrap_or_else(|| json!({
            "input_tokens":collected.usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            "output_tokens":collected.usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0)
        }))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn map_stream(source: &str) -> String {
        let upstream: ResponseStream = futures_util::stream::iter(vec![Ok::<Bytes, Infallible>(
            Bytes::copy_from_slice(source.as_bytes()),
        )])
        .boxed();
        let mut mapped = responses_to_anthropic_stream(upstream, "mapped-model".to_owned(), false);
        let mut output = Vec::new();
        while let Some(chunk) = mapped.next().await {
            output.extend_from_slice(&chunk.unwrap());
        }
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn converts_anthropic_tools_and_images_to_responses() {
        let request = MessageRequest {
            model: "backend".to_owned(),
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            top_k: None,
            stream: true,
            speed: None,
            messages: vec![crate::anthropic::Message {
                role: "user".to_owned(),
                content: vec![
                    ContentBlock::Text {
                        text: "look".to_owned(),
                    },
                    ContentBlock::Image {
                        source: json!({"type":"base64","media_type":"image/png","data":"AAAA"}),
                    },
                ],
            }],
            system: Some(vec![ContentBlock::Text {
                text: "system".to_owned(),
            }]),
            tools: vec![
                json!({"name":"bash","description":"run","input_schema":{"type":"object"}}),
            ],
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        };

        let body = message_request_to_responses(&request, "backend-provider").unwrap();

        assert_eq!(body["instructions"], "system");
        assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");
    }

    #[tokio::test]
    async fn incomplete_token_limit_maps_to_max_tokens() {
        let output = map_stream(
            "event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_1\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":3,\"output_tokens\":7}}}\n\n",
        )
        .await;

        assert!(output.contains("\"stop_reason\":\"max_tokens\""));
        assert!(output.contains("event: message_stop"));
        assert!(!output.contains("event: error"));
    }

    #[tokio::test]
    async fn truncated_responses_stream_maps_to_anthropic_error() {
        let output = map_stream(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        )
        .await;

        assert!(output.contains("event: error"));
        assert!(output.contains("upstream response ended before completion"));
        assert!(!output.contains("event: message_stop"));
    }

    #[test]
    fn buffered_token_limit_keeps_max_tokens_stop_reason() {
        let message = collected_to_anthropic_message(
            CollectedResponse {
                response: json!({
                    "id":"resp_1",
                    "status":"incomplete",
                    "incomplete_details":{"reason":"max_output_tokens"}
                }),
                output: Vec::new(),
                output_text: String::new(),
                usage: json!({"input_tokens":3,"output_tokens":7}),
            },
            "mapped-model",
        )
        .unwrap();

        assert_eq!(message["stop_reason"], "max_tokens");
        assert_eq!(message["usage"]["output_tokens"], 7);
    }
}
