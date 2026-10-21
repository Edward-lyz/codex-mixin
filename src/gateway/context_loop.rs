use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::GatewayExecutor;
use super::plan::RequestPlan;
use crate::context::ContextEngine;
use crate::context::guidance::with_guidance;
use crate::context::types::{SESSION_SEPARATOR, is_context_tool};
use crate::error::GatewayError;
use crate::protocol::ResponseStream;
use crate::protocol::sse::{SseDecoder, encode_event, encode_raw_event, response_failed_payload};

const CLIENT_ACTION_TYPES: [&str; 7] = [
    "function_call",
    "custom_tool_call",
    "local_shell_call",
    "computer_call",
    "code_interpreter_call",
    "image_generation_call",
    "mcp_approval_request",
];

const ROUNDS_EXHAUSTED: &str = "context internal rounds exhausted before the model answered";

/// A request that participates in context management.
///
/// The run owns the session identity, the context engine and the limits for
/// internal continuation rounds. It replaces the client input with the
/// compressed view, injects the internal tools and keeps the whole exchange
/// hidden from the client.
pub(crate) struct ContextRun {
    engine: Arc<ContextEngine>,
    session: String,
    window_tokens: u64,
    max_internal_rounds: usize,
}

struct RoundOutcome {
    chunks: Vec<Bytes>,
    completed: Option<Value>,
    failed: bool,
}

impl ContextRun {
    /// Returns None when context management does not apply, so the caller keeps
    /// its existing request path.
    pub(crate) fn prepare(
        executor: &GatewayExecutor,
        body: &Value,
        headers: &HeaderMap,
    ) -> Result<Option<Self>, GatewayError> {
        let Some(engine) = executor.context_engine() else {
            return Ok(None);
        };
        if !matches!(body.get("input"), Some(Value::Array(_))) {
            tracing::debug!("context engine skips a request whose input is not an item array");
            return Ok(None);
        }
        let Some(session) = context_session(executor, headers, body)? else {
            tracing::debug!("context engine skips a request without a session identity");
            return Ok(None);
        };
        let settings = engine.settings();
        let window_tokens = settings.context_window_tokens.unwrap_or(0);
        let max_internal_rounds = settings.max_internal_rounds;
        Ok(Some(Self {
            engine,
            session,
            window_tokens,
            max_internal_rounds,
        }))
    }

    pub(crate) async fn stream(
        self,
        executor: GatewayExecutor,
        plan: RequestPlan,
        headers: HeaderMap,
    ) -> Result<ResponseStream, GatewayError> {
        let original = plan
            .body
            .get("input")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let lease = self
            .engine
            .clone()
            .acquire_lease(&self.session)
            .map_err(|error| GatewayError::BadRequest(error.to_string()))?;
        let view = self
            .engine
            .begin_turn(&self.session, &original, self.window_tokens)?;
        tracing::info!(
            session = %crate::context::types::session_display_name(&self.session),
            revision = view.revision,
            estimated_view_tokens = view.estimated_tokens,
            window_tokens = view.window_tokens,
            "prepared context view"
        );
        let template = plan.body.clone();
        let stream = async_stream::stream! {
            // The lease lives exactly as long as the client consumes this turn.
            let _lease = lease;
            let mut base = view.items;
            let mut appended: Vec<Value> = Vec::new();
            let mut revision = view.revision;
            let mut internal_rounds = 0usize;
            loop {
                let body = compose_body(&template, &base, &appended, &self.engine);
                let mut round_plan = plan.clone();
                round_plan.body = body;
                let outcome = match run_round(&executor, round_plan, &headers).await {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        yield Ok::<Bytes, Infallible>(failure_event(&template, error.to_string()));
                        return;
                    }
                };
                if outcome.failed {
                    for chunk in outcome.chunks {
                        yield Ok(chunk);
                    }
                    return;
                }
                let Some(completed) = outcome.completed else {
                    for chunk in outcome.chunks {
                        yield Ok(chunk);
                    }
                    return;
                };
                let output = completed
                    .get("output")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let internal: Vec<Value> = output
                    .iter()
                    .filter(|item| internal_tool_name(item).is_some())
                    .cloned()
                    .collect();
                if internal.is_empty() {
                    for chunk in hidden_round(outcome.chunks, &template) {
                        yield Ok(chunk);
                    }
                    return;
                }
                let hand_back = output.iter().any(is_client_action);
                if !hand_back && internal_rounds >= self.max_internal_rounds {
                    yield Ok::<Bytes, Infallible>(failure_event(
                        &template,
                        ROUNDS_EXHAUSTED.to_owned(),
                    ));
                    return;
                }
                if !hand_back {
                    internal_rounds += 1;
                }
                let mut calls: Vec<Value> = Vec::new();
                let mut outputs: Vec<Value> = Vec::new();
                let mut broken = None;
                for item in &internal {
                    let name = internal_tool_name(item).unwrap_or_default();
                    let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
                        broken = Some(format!("context tool {name} call has no call_id"));
                        break;
                    };
                    let arguments = tool_arguments(item);
                    let execution = self
                        .engine
                        .execute_tool(&self.session, revision, name, &arguments, self.window_tokens);
                    let (content, next_revision) = match execution {
                        Ok(execution) => (execution.content, execution.revision),
                        Err(error) => {
                            if hand_back {
                                tracing::warn!(
                                    tool = name,
                                    error = %error,
                                    "context tool failed while the model also asked for client work"
                                );
                            }
                            (
                                json!({"ok": false, "error": error.to_string()}).to_string(),
                                revision,
                            )
                        }
                    };
                    let call = normalized_call(item, call_id);
                    outputs.push(tool_output_item(&call, content));
                    calls.push(call);
                    revision = next_revision;
                }
                if let Some(message) = broken {
                    yield Ok::<Bytes, Infallible>(failure_event(&template, message));
                    return;
                }
                if hand_back {
                    // The model asked for client work and for context work in
                    // one response: commit the context results, then deliver
                    // only the client calls. The model resumes after the client
                    // returns its tool outputs.
                    for chunk in hidden_round(outcome.chunks, &template) {
                        yield Ok(chunk);
                    }
                    return;
                }
                let view = match self
                    .engine
                    .begin_turn(&self.session, &original, self.window_tokens)
                {
                    Ok(view) => view,
                    Err(error) => {
                        yield Ok::<Bytes, Infallible>(failure_event(&template, error.to_string()));
                        return;
                    }
                };
                base = view.items;
                revision = view.revision;
                appended.extend(calls);
                appended.extend(outputs);
            }
        };
        Ok(stream.boxed())
    }
}

async fn run_round(
    executor: &GatewayExecutor,
    plan: RequestPlan,
    headers: &HeaderMap,
) -> Result<RoundOutcome, GatewayError> {
    let (mut stream, _) = executor.send_round(plan, headers).await?;
    let mut decoder = SseDecoder::default();
    let mut chunks = Vec::new();
    let mut completed = None;
    let mut failed = false;
    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(never) => match never {},
        };
        for event in decoder.push(&bytes) {
            match event.event.as_deref() {
                Some("response.completed") => {
                    completed = serde_json::from_str::<Value>(&event.data)
                        .ok()
                        .and_then(|payload| payload.get("response").cloned());
                }
                Some("response.failed") => failed = true,
                _ => {}
            }
        }
        chunks.push(bytes);
    }
    Ok(RoundOutcome {
        chunks,
        completed,
        failed,
    })
}

/// Removes the internal tool calls and their output from the round the client
/// receives, so context work leaves no trace in the answer.
fn hidden_round(chunks: Vec<Bytes>, template: &Value) -> Vec<Bytes> {
    let mut decoder = SseDecoder::default();
    let mut hidden_items: HashSet<String> = HashSet::new();
    let mut hidden_calls: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for chunk in chunks {
        for event in decoder.push(&chunk) {
            if let Some(bytes) =
                visible_event(&event, template, &mut hidden_items, &mut hidden_calls)
            {
                out.push(bytes);
            }
        }
    }
    let rest = decoder.remaining();
    if !rest.is_empty() {
        out.push(Bytes::copy_from_slice(rest));
    }
    out
}

fn visible_event(
    event: &crate::protocol::sse::SseEvent,
    template: &Value,
    hidden_items: &mut HashSet<String>,
    hidden_calls: &mut HashSet<String>,
) -> Option<Bytes> {
    let Ok(mut payload) = serde_json::from_str::<Value>(&event.data) else {
        return Some(encode_sse(event.event.as_deref(), &event.data));
    };
    if let Some(item) = payload.get("item")
        && internal_tool_name(item).is_some()
    {
        remember_hidden_item(item, hidden_items, hidden_calls);
        return None;
    }
    let hidden_item = payload
        .get("item_id")
        .and_then(Value::as_str)
        .is_some_and(|id| hidden_items.contains(id));
    let hidden_call = payload
        .get("call_id")
        .and_then(Value::as_str)
        .is_some_and(|id| hidden_calls.contains(id));
    if hidden_item || hidden_call {
        return None;
    }
    if let Some(response) = payload.get_mut("response").and_then(Value::as_object_mut) {
        if let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) {
            output.retain(|item| internal_tool_name(item).is_none());
        }
        if let Some(tools) = response.get_mut("tools").and_then(Value::as_array_mut) {
            tools.retain(|tool| !tool_name(tool).is_some_and(is_context_tool));
        }
        if response.contains_key("instructions") {
            let instructions = template.get("instructions").cloned().unwrap_or(Value::Null);
            response.insert("instructions".to_owned(), instructions);
        }
    }
    Some(encode_sse(event.event.as_deref(), &payload.to_string()))
}

fn remember_hidden_item(
    item: &Value,
    hidden_items: &mut HashSet<String>,
    hidden_calls: &mut HashSet<String>,
) {
    if let Some(id) = item.get("id").and_then(Value::as_str) {
        hidden_items.insert(id.to_owned());
    }
    if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
        hidden_calls.insert(call_id.to_owned());
    }
}

fn encode_sse(event: Option<&str>, data: &str) -> Bytes {
    match event {
        Some(name) => encode_raw_event(name, data),
        None => {
            let mut encoded = Vec::with_capacity(data.len() + 8);
            encoded.extend_from_slice(b"data: ");
            encoded.extend_from_slice(data.as_bytes());
            encoded.extend_from_slice(b"\n\n");
            Bytes::from(encoded)
        }
    }
}

fn compose_body(
    template: &Value,
    base: &[Value],
    appended: &[Value],
    engine: &ContextEngine,
) -> Value {
    let mut body = template.clone();
    let mut input: Vec<Value> = base.to_vec();
    input.extend(appended.iter().cloned());
    body["input"] = Value::Array(input);
    body["instructions"] = Value::String(with_guidance(
        body.get("instructions").and_then(Value::as_str),
    ));
    if let Some(object) = body.as_object_mut() {
        object.remove("previous_response_id");
    }
    let mut tools: Vec<Value> = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    tools.retain(|tool| !tool_name(tool).is_some_and(is_context_tool));
    tools.extend(engine.tool_definitions());
    body["tools"] = Value::Array(tools);
    body
}

fn tool_name(tool: &Value) -> Option<&str> {
    tool.get("name")
        .and_then(Value::as_str)
        .or_else(|| tool.pointer("/function/name").and_then(Value::as_str))
}

fn is_tool_call(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call" | "custom_tool_call")
    )
}

fn internal_tool_name(item: &Value) -> Option<&str> {
    if !is_tool_call(item) {
        return None;
    }
    item.get("name")
        .and_then(Value::as_str)
        .filter(|name| is_context_tool(name))
}

fn is_client_action(item: &Value) -> bool {
    if internal_tool_name(item).is_some() {
        return false;
    }
    item.get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| CLIENT_ACTION_TYPES.contains(&kind))
}

fn tool_arguments(item: &Value) -> Value {
    match item.get("arguments").or_else(|| item.get("input")) {
        Some(Value::String(value)) if value.trim().is_empty() => Value::Null,
        Some(Value::String(value)) => serde_json::from_str(value).unwrap_or(Value::Null),
        Some(value) => value.clone(),
        None => Value::Null,
    }
}

fn normalized_call(item: &Value, call_id: &str) -> Value {
    let mut call = json!({
        "type": item.get("type").cloned().unwrap_or(Value::Null),
        "call_id": call_id,
        "name": item.get("name").cloned().unwrap_or(Value::Null),
    });
    if let Some(arguments) = item.get("arguments") {
        call["arguments"] = arguments.clone();
    }
    if let Some(input) = item.get("input") {
        call["input"] = input.clone();
    }
    call
}

fn tool_output_item(call: &Value, content: String) -> Value {
    let kind = match call.get("type").and_then(Value::as_str) {
        Some("custom_tool_call") => "custom_tool_call_output",
        _ => "function_call_output",
    };
    json!({
        "type": kind,
        "call_id": call.get("call_id").cloned().unwrap_or(Value::Null),
        "output": content,
    })
}

fn failure_event(body: &Value, message: String) -> Bytes {
    let model = body.get("model").and_then(Value::as_str);
    match encode_event(
        "response.failed",
        &response_failed_payload(None, model, message, "server_error"),
    ) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(error = %error, "failed to encode context failure event");
            Bytes::new()
        }
    }
}

/// Session identity is the authenticated client plus the explicit thread or
/// session the client sent. Subagent routing hashes are included so a subagent
/// never shares the parent view.
fn context_session(
    executor: &GatewayExecutor,
    headers: &HeaderMap,
    body: &Value,
) -> Result<Option<String>, GatewayError> {
    let Some(routing) = super::stable_oneapi_routing(headers, body)? else {
        return Ok(None);
    };
    let client = executor.client_identity(headers);
    Ok(Some(format!(
        "{client}{SESSION_SEPARATOR}{}",
        routing.hash_key
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContextSettings;

    fn engine() -> ContextEngine {
        ContextEngine::open_in_memory(ContextSettings {
            enabled: true,
            ..ContextSettings::default()
        })
        .expect("in-memory context engine")
    }

    fn user(text: &str) -> Value {
        json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]})
    }

    #[test]
    fn replaces_input_and_injects_internal_tools() {
        let engine = engine();
        let template = json!({
            "model": "test",
            "instructions": "be helpful",
            "input": [user("hello")],
            "previous_response_id": "resp_1",
            "tools": [{"type": "function", "name": "shell", "parameters": {}}],
        });
        let base = vec![user("compressed view")];
        let body = compose_body(&template, &base, &[], &engine);
        assert_eq!(body["input"], json!([user("compressed view")]));
        assert!(body.get("previous_response_id").is_none());
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 5);
        assert_eq!(tools[0]["name"], "shell");
        for tool in &tools[1..] {
            assert!(is_context_tool(tool["name"].as_str().unwrap_or_default()));
        }
        assert!(
            body["instructions"]
                .as_str()
                .unwrap_or_default()
                .contains("<context_management>")
        );
    }

    #[test]
    fn replaces_client_tools_that_shadow_internal_names() {
        let engine = engine();
        let template = json!({"tools": [{"type": "function", "name": "mixin_context_status"}]});
        let body = compose_body(&template, &[], &[], &engine);
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 4);
    }

    fn raw(event: &str, payload: &Value) -> Bytes {
        encode_raw_event(event, &payload.to_string())
    }

    #[test]
    fn hides_internal_calls_from_the_client_round() {
        let template = json!({"instructions": "be helpful"});
        let internal = json!({"type": "function_call", "id": "fc_internal", "call_id": "call_internal", "name": "mixin_context_status", "arguments": "{}"});
        let client = json!({"type": "function_call", "id": "fc_client", "call_id": "call_client", "name": "exec_command", "arguments": "{}"});
        let chunks = vec![
            raw("response.output_item.added", &json!({"item": internal})),
            raw(
                "response.function_call_arguments.delta",
                &json!({"item_id": "fc_internal", "delta": "{}"}),
            ),
            raw("response.output_item.added", &json!({"item": client})),
            raw(
                "response.completed",
                &json!({"response": {
                    "output": [internal, client],
                    "tools": [{"type": "function", "name": "mixin_context_status"}],
                    "instructions": "guided",
                }}),
            ),
        ];
        let visible = hidden_round(chunks, &template);
        let text = String::from_utf8_lossy(&visible.concat()).to_string();
        assert!(
            !text.contains("call_internal"),
            "internal call leaked: {text}"
        );
        assert!(
            !text.contains("fc_internal"),
            "internal deltas leaked: {text}"
        );
        assert!(
            text.contains("call_client"),
            "client call was hidden: {text}"
        );
        let completed = visible
            .iter()
            .find_map(|chunk| {
                let chunk = String::from_utf8_lossy(chunk).to_string();
                let data = chunk.lines().find(|line| line.starts_with("data: "))?;
                let payload: Value =
                    serde_json::from_str(data.trim_start_matches("data: ")).ok()?;
                payload.get("response").cloned()
            })
            .expect("completed payload");
        let output = completed["output"].as_array().expect("output");
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["name"], "exec_command");
        assert!(completed["tools"].as_array().expect("tools").is_empty());
        assert_eq!(completed["instructions"], "be helpful");
    }

    #[test]
    fn classifies_internal_calls_and_client_actions() {
        let internal = json!({"type": "function_call", "call_id": "c1", "name": "mixin_context_search", "arguments": "{}"});
        assert_eq!(internal_tool_name(&internal), Some("mixin_context_search"));
        assert!(!is_client_action(&internal));
        let shell =
            json!({"type": "function_call", "call_id": "c2", "name": "shell", "arguments": "{}"});
        assert!(internal_tool_name(&shell).is_none());
        assert!(is_client_action(&shell));
        let message = json!({"type": "message", "role": "assistant", "content": []});
        assert!(!is_client_action(&message));
    }

    #[test]
    fn parses_arguments_from_function_and_custom_calls() {
        assert_eq!(
            tool_arguments(&json!({"arguments": "{\"query\":\"alpha\"}"})),
            json!({"query": "alpha"})
        );
        assert_eq!(
            tool_arguments(&json!({"input": "{\"block_id\":\"b000001\"}"})),
            json!({"block_id": "b000001"})
        );
        assert_eq!(tool_arguments(&json!({"arguments": ""})), Value::Null);
        assert_eq!(tool_arguments(&json!({})), Value::Null);
    }

    #[test]
    fn builds_matching_tool_output_items() {
        let call = normalized_call(
            &json!({"type": "custom_tool_call", "name": "mixin_context_status", "input": "{}"}),
            "c9",
        );
        assert_eq!(call["call_id"], "c9");
        assert_eq!(call["input"], "{}");
        let output = tool_output_item(&call, "{\"ok\":true}".to_owned());
        assert_eq!(output["type"], "custom_tool_call_output");
        assert_eq!(output["call_id"], "c9");
    }

    #[test]
    fn encodes_explicit_failure_events() {
        let bytes = failure_event(&json!({"model": "test"}), "boom".to_owned());
        let text = String::from_utf8(bytes.to_vec()).expect("utf8 event");
        assert!(text.contains("event: response.failed"));
        assert!(text.contains("\"message\":\"boom\""));
    }
}
