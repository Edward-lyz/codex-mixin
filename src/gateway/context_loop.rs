use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::GatewayExecutor;
use super::plan::RequestPlan;
use super::turn_usage::unix_millis;
use crate::context::ContextEngine;
use crate::context::TurnSample;
use crate::context::checkpoint;
use crate::context::guidance::{GUIDANCE, GUIDANCE_WITHOUT_TOOLS, with_guidance};
use crate::context::types::{SESSION_SEPARATOR, TOOL_COMPRESS, TOOL_DECOMPRESS, is_context_tool};
use crate::error::GatewayError;
use crate::protocol::ResponseStream;
use crate::protocol::sse::{SseDecoder, encode_raw_event, response_failed_payload};

const CLIENT_ACTION_TYPES: [&str; 7] = [
    "function_call",
    "custom_tool_call",
    "local_shell_call",
    "computer_call",
    "code_interpreter_call",
    "image_generation_call",
    "mcp_approval_request",
];

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
    enabled: bool,
    guidance_only: bool,
}

impl ContextRun {
    /// Returns None when context management does not apply, so the caller keeps
    /// its existing request path.
    pub(crate) async fn prepare(
        executor: &GatewayExecutor,
        body: &Value,
        headers: &HeaderMap,
    ) -> Result<Option<Self>, GatewayError> {
        let Some(engine) = executor.context_engine() else {
            return Ok(None);
        };
        let has_items = matches!(body.get("input"), Some(Value::Array(_)));
        // A chained request may carry only the delta, so it still belongs to
        // the session even without an item array.
        let chained = body
            .get("previous_response_id")
            .is_some_and(Value::is_string);
        if !has_items && !chained {
            tracing::debug!("context engine skips a request whose input is not an item array");
            return Ok(None);
        }
        let enabled = engine.is_enabled();
        let checkpoint = body
            .get("input")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(checkpoint::is_checkpoint_item));
        if !enabled && !checkpoint {
            // With the feature off the client keeps the existing request path,
            // but a session that already holds checkpoints stays readable.
            tracing::debug!("context engine skips a request while the feature is off");
            return Ok(None);
        }
        let Some(session) = context_session(executor, headers, body)? else {
            tracing::debug!("context engine skips a request without a session identity");
            return Ok(None);
        };
        let settings = engine.settings();
        let window_tokens = match settings.context_window_tokens {
            Some(window) => window,
            None => {
                let model = body
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                executor.catalog.context_window(model).await?
            }
        };
        let max_internal_rounds = settings.max_internal_rounds;
        let guidance_only = settings.guidance_only;
        Ok(Some(Self {
            engine,
            session,
            window_tokens,
            max_internal_rounds,
            enabled,
            guidance_only,
        }))
    }

    pub(crate) async fn stream(
        self,
        executor: GatewayExecutor,
        plan: RequestPlan,
        headers: HeaderMap,
    ) -> Result<ResponseStream, GatewayError> {
        let lease = self
            .engine
            .clone()
            .acquire_lease(&self.session)
            .map_err(|error| GatewayError::BadRequest(error.to_string()))?;
        let original = context_input(&self.engine, &self.session, &plan.body)?;
        if !self.enabled {
            return self
                .stream_read_only(executor, plan, headers, original, lease)
                .await;
        }
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
        // The model can only time its own compression when the request says how
        // full the view is, so every round carries the current pressure.
        let mut note = pressure_note(&self.engine, &self.session, &view, self.guidance_only)?;
        // The first round runs before the response starts, so an upstream status
        // or transport error keeps its own status instead of reaching the client
        // as a successful stream that only carries a failure event.
        let mut first_plan = plan.clone();
        first_plan.body = compose_body(
            &template,
            &view.items,
            &[],
            &self.engine,
            !self.guidance_only,
            &note,
        );
        let (first, _) = executor.send_round(first_plan, &headers).await?;
        let stream = async_stream::stream! {
            // The lease lives exactly as long as the client consumes this turn.
            let _lease = lease;
            let mut base = view.items;
            let mut appended: Vec<Value> = Vec::new();
            let mut revision = view.revision;
            let mut internal_rounds = 0usize;
            let mut failed_signatures = HashSet::new();
            let mut pending = Some(first);
            let mut client = ContextEvents::default();
            loop {
                let mut upstream = match pending.take() {
                    Some(outcome) => outcome,
                    None => {
                        let body = compose_body(
                            &template,
                            &base,
                            &appended,
                            &self.engine,
                            !self.guidance_only,
                            &note,
                        );
                        let mut round_plan = plan.clone();
                        round_plan.body = body;
                        match executor.send_round(round_plan, &headers).await {
                            Ok((stream, _)) => stream,
                            Err(error) => {
                                yield Ok::<Bytes, Infallible>(client.failure(
                                    &template,
                                    error.to_string(),
                                ));
                                return;
                            }
                        }
                    }
                };
                client.begin_round();
                let mut decoder = SseDecoder::default();
                let mut completed = None;
                while completed.is_none() {
                    let Some(chunk) = upstream.next().await else { break };
                    let bytes = match chunk { Ok(bytes) => bytes, Err(never) => match never {} };
                    for mut event in decoder.push(&bytes) {
                        // Responses providers may send data-only SSE carrying
                        // the event type in JSON instead of an event line.
                        if event.event.is_none() {
                            event.event = serde_json::from_str::<Value>(&event.data).ok()
                                .and_then(|payload| payload.get("type").and_then(Value::as_str).map(str::to_owned));
                        }
                        if event.event.as_deref() == Some("response.completed") {
                            completed = serde_json::from_str::<Value>(&event.data).ok()
                                .and_then(|payload| payload.get("response").cloned());
                            break;
                        }
                        if event.data == "[DONE]" { continue; }
                        match client.render(&event, &template) {
                            Ok(Some(bytes)) => yield Ok::<Bytes, Infallible>(bytes),
                            Ok(None) => {},
                            Err(error) => {
                                yield Ok(client.failure(&template, error.to_string()));
                                return;
                            }
                        }
                        if matches!(event.event.as_deref(), Some("response.failed" | "response.incomplete")) {
                            return;
                        }
                    }
                }
                let Some(mut completed) = completed else {
                    yield Ok(client.failure(&template, "context upstream ended without a completed response".to_owned()));
                    return;
                };
                let output = completed
                    .get("output")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.record_usage(&completed);
                client.output.extend(output.iter().filter(|item| internal_tool_name(item).is_none()).cloned());
                let internal: Vec<Value> = output
                    .iter()
                    .filter(|item| internal_tool_name(item).is_some())
                    .cloned()
                    .collect();
                let finish_turn = internal.is_empty() || output.iter().any(is_client_action);
                if !finish_turn { internal_rounds += 1; }
                let mut outputs: Vec<Value> = Vec::new();
                let mut broken = None;
                let mut failed_mutations = 0usize;
                let mut repeated_failure = false;
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
                            if matches!(name, TOOL_COMPRESS | TOOL_DECOMPRESS) {
                                failed_mutations += 1;
                                let signature = (name.to_owned(), arguments.to_string());
                                repeated_failure |= !failed_signatures.insert(signature);
                            }
                            if finish_turn {
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
                    revision = next_revision;
                }
                if let Some(message) = broken {
                    yield Ok::<Bytes, Infallible>(client.failure(&template, message));
                    return;
                }
                let limit_reached = !finish_turn && internal_rounds >= self.max_internal_rounds;
                if limit_reached {
                    tracing::warn!(internal_rounds, "context loop limit reached; completing gracefully");
                }
                let failure_loop = !finish_turn && repeated_failure && failed_mutations == internal.len();
                if failure_loop {
                    tracing::warn!(internal_rounds, "repeated identical context mutation failure; completing gracefully");
                }
                if finish_turn || limit_reached || failure_loop {
                    // Client actions become visible only after internal commits.
                    client.complete(&mut completed, &template);
                    self.record_completed(&completed, revision, &client.output);
                    for chunk in client.drain_actions() { yield Ok(chunk); }
                    yield Ok(client.encode(Some("response.completed"), json!({"type":"response.completed", "response":completed})));
                    return;
                }
                let view = match self
                    .engine
                    .begin_turn(&self.session, &original, self.window_tokens)
                {
                    Ok(view) => view,
                    Err(error) => {
                        yield Ok::<Bytes, Infallible>(client.failure(&template, error.to_string()));
                        return;
                    }
                };
                note = match pressure_note(&self.engine, &self.session, &view, self.guidance_only) {
                    Ok(note) => note,
                    Err(error) => { yield Ok(client.failure(&template, error.to_string())); return; }
                };
                base = view.items;
                revision = view.revision;
                appended.extend(output);
                appended.extend(outputs);
            }
        };
        Ok(stream.boxed())
    }

    /// Serves a session whose checkpoints still have to be expanded while the
    /// feature is off. The view is read from the archive, no internal tools are
    /// injected, nothing is recorded and the model gets a single round.
    async fn stream_read_only(
        self,
        executor: GatewayExecutor,
        plan: RequestPlan,
        headers: HeaderMap,
        original: Vec<Value>,
        lease: crate::context::engine::Lease,
    ) -> Result<ResponseStream, GatewayError> {
        let view = self
            .engine
            .begin_turn(&self.session, &original, self.window_tokens)
            .map_err(|error| GatewayError::BadRequest(error.to_string()))?;
        tracing::info!(
            session = %crate::context::types::session_display_name(&self.session),
            revision = view.revision,
            "expanded a context checkpoint with the feature off"
        );
        let template = plan.body.clone();
        let mut read_plan = plan;
        read_plan.body = compose_read_only_body(&template, &view.items);
        let (mut upstream, _) = executor.send_round(read_plan, &headers).await?;
        Ok(async_stream::stream! {
            let _lease = lease;
            while let Some(chunk) = upstream.next().await {
                yield chunk;
            }
        }
        .boxed())
    }

    /// Records the billed usage of one upstream round. A round without usage is
    /// skipped instead of being counted as a free round.
    fn record_usage(&self, completed: &Value) {
        let Some(usage) = completed.get("usage") else {
            return;
        };
        let Some(input_tokens) = usage.get("input_tokens").and_then(Value::as_u64) else {
            return;
        };
        let sample = TurnSample {
            at_ms: unix_millis() as i64,
            input_tokens,
            cached_tokens: usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        };
        if let Err(error) = self.engine.record_turn(&self.session, sample) {
            tracing::warn!(error = %error, "failed to record context turn usage");
        }
    }

    /// Stores the mapping a chained follow-up request depends on. The client
    /// only ever saw the visible items, so internal calls are left out.
    fn record_completed(&self, completed: &Value, revision: i64, output: &[Value]) {
        let Some(response_id) = completed.get("id").and_then(Value::as_str) else {
            return;
        };
        let visible: Vec<Value> = output
            .iter()
            .filter(|item| internal_tool_name(item).is_none())
            .cloned()
            .collect();
        if let Err(error) =
            self.engine
                .record_response(&self.session, response_id, revision, &visible)
        {
            tracing::warn!(
                response_id,
                error = %error,
                "failed to record the context response mapping"
            );
        }
    }
}

// Only client tool events wait for internal commits. Text stays incremental,
// and the held events have a fixed memory ceiling rather than buffering a turn.
const MAX_PENDING_TOOL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct ContextEvents {
    response_id: Option<String>,
    started: bool,
    sequence: u64,
    next_index: u64,
    indices: HashMap<u64, u64>,
    hidden_items: HashSet<String>,
    hidden_calls: HashSet<String>,
    action_items: HashSet<String>,
    action_calls: HashSet<String>,
    pending_actions: Vec<(Option<String>, Value)>,
    pending_bytes: usize,
    output: Vec<Value>,
}

impl ContextEvents {
    fn begin_round(&mut self) {
        self.indices.clear();
        self.hidden_items.clear();
        self.hidden_calls.clear();
        self.action_items.clear();
        self.action_calls.clear();
        self.pending_bytes = 0;
    }

    fn complete(&mut self, response: &mut Value, template: &Value) {
        if let Some(id) = &self.response_id {
            response["id"] = json!(id);
        }
        response["output"] = json!(self.output);
        if let Some(tools) = response.get_mut("tools").and_then(Value::as_array_mut) {
            tools.retain(|tool| !tool_name(tool).is_some_and(is_context_tool));
        }
        if response.get("instructions").is_some() {
            response["instructions"] = template.get("instructions").cloned().unwrap_or(Value::Null);
        }
    }

    fn render(
        &mut self,
        event: &crate::protocol::sse::SseEvent,
        template: &Value,
    ) -> Result<Option<Bytes>, GatewayError> {
        let Some(mut payload) = visible_payload(
            event,
            template,
            &mut self.hidden_items,
            &mut self.hidden_calls,
        ) else {
            return Ok(None);
        };
        if event.event.as_deref() == Some("response.created") {
            if self.started {
                return Ok(None);
            }
            self.started = true;
        }
        if let Some(response) = payload.get_mut("response").and_then(Value::as_object_mut) {
            if self.response_id.is_none() {
                self.response_id = response
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            if let Some(id) = &self.response_id {
                response.insert("id".to_owned(), json!(id));
            }
        }
        if payload.get("response_id").is_some()
            && let Some(id) = &self.response_id
        {
            payload["response_id"] = json!(id);
        }
        if let Some(index) = payload.get("output_index").and_then(Value::as_u64) {
            let next = self.indices.entry(index).or_insert_with(|| {
                let next = self.next_index;
                self.next_index += 1;
                next
            });
            payload["output_index"] = json!(next);
        }
        let action = payload.get("item").is_some_and(is_client_action);
        if action && let Some(item) = payload.get("item") {
            remember_hidden_item(item, &mut self.action_items, &mut self.action_calls);
        }
        let held = action
            || payload
                .get("item_id")
                .and_then(Value::as_str)
                .is_some_and(|id| self.action_items.contains(id))
            || payload
                .get("call_id")
                .and_then(Value::as_str)
                .is_some_and(|id| self.action_calls.contains(id));
        if held {
            self.pending_bytes = self.pending_bytes.saturating_add(event.data.len());
            if self.pending_bytes > MAX_PENDING_TOOL_BYTES {
                return Err(GatewayError::Other(anyhow::anyhow!(
                    "context client tool events exceed the buffer limit"
                )));
            }
            self.pending_actions.push((event.event.clone(), payload));
            return Ok(None);
        }
        Ok(Some(self.encode(event.event.as_deref(), payload)))
    }

    fn failure(&mut self, body: &Value, message: String) -> Bytes {
        let model = body.get("model").and_then(Value::as_str);
        self.encode(
            Some("response.failed"),
            response_failed_payload(self.response_id.clone(), model, message, "server_error"),
        )
    }

    fn encode(&mut self, event: Option<&str>, mut payload: Value) -> Bytes {
        payload["sequence_number"] = json!(self.sequence);
        self.sequence += 1;
        encode_sse(event, &payload.to_string())
    }

    fn drain_actions(&mut self) -> Vec<Bytes> {
        std::mem::take(&mut self.pending_actions)
            .into_iter()
            .map(|(event, payload)| self.encode(event.as_deref(), payload))
            .collect()
    }
}

/// Removes the internal tool calls and their output from the round the client
/// receives, so context work leaves no trace in the answer.
#[cfg(test)]
fn hidden_round(chunks: Vec<Bytes>, template: &Value) -> Vec<Bytes> {
    let mut decoder = SseDecoder::default();
    let mut hidden_items: HashSet<String> = HashSet::new();
    let mut hidden_calls: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for chunk in chunks {
        for event in decoder.push(&chunk) {
            if let Some(payload) =
                visible_payload(&event, template, &mut hidden_items, &mut hidden_calls)
            {
                out.push(encode_sse(event.event.as_deref(), &payload.to_string()));
            }
        }
    }
    let rest = decoder.remaining();
    if !rest.is_empty() {
        out.push(Bytes::copy_from_slice(rest));
    }
    out
}

fn visible_payload(
    event: &crate::protocol::sse::SseEvent,
    template: &Value,
    hidden_items: &mut HashSet<String>,
    hidden_calls: &mut HashSet<String>,
) -> Option<Value> {
    let mut payload = serde_json::from_str::<Value>(&event.data).ok()?;
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
    Some(payload)
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
    inject_tools: bool,
    note: &str,
) -> Value {
    let mut body = template.clone();
    let mut input: Vec<Value> = base.to_vec();
    input.extend(appended.iter().cloned());
    // The state rides at the end of the input instead of the instructions: an
    // instruction that changes every round would invalidate the provider prefix
    // cache, while an appended message only extends it.
    if !note.is_empty() {
        input.push(json!({
            "type": "message",
            "role": "developer",
            "content": [{"type": "input_text", "text": format!("<context_state>\n{note}</context_state>")}],
        }));
    }
    body["input"] = Value::Array(input);
    let guidance = if inject_tools {
        GUIDANCE
    } else {
        GUIDANCE_WITHOUT_TOOLS
    };
    body["instructions"] = Value::String(with_guidance(
        body.get("instructions").and_then(Value::as_str),
        guidance,
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
    if inject_tools {
        tools.extend(engine.tool_definitions());
    }
    body["tools"] = Value::Array(tools);
    body
}

/// Reports how full the view is and which range is safe to fold next.
fn pressure_note(
    engine: &ContextEngine,
    session: &str,
    view: &crate::context::TurnView,
    guidance_only: bool,
) -> Result<String, GatewayError> {
    engine
        .pressure_note(session, view, !guidance_only)
        .map_err(Into::into)
}

/// Replaces the input with a view read from the archive without injecting the
/// internal tools, so a disabled session keeps its compressed history.
fn compose_read_only_body(template: &Value, items: &[Value]) -> Value {
    let mut body = template.clone();
    body["input"] = Value::Array(items.to_vec());
    if let Some(object) = body.as_object_mut() {
        object.remove("previous_response_id");
    }
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

/// Restores the same client-visible history for ordinary turns and compaction.
/// A checkpoint must be authenticated and expanded before syncing the archive.
pub(crate) fn context_input(
    engine: &ContextEngine,
    session: &str,
    body: &Value,
) -> Result<Vec<Value>, GatewayError> {
    let incoming = match body.get("input") {
        Some(Value::Array(items)) => items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) != Some("compaction_trigger"))
            .cloned()
            .collect(),
        Some(Value::String(text)) => vec![json!({
            "type": "message", "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })],
        None if body
            .get("previous_response_id")
            .is_some_and(Value::is_string) =>
        {
            Vec::new()
        }
        _ => {
            return Err(GatewayError::BadRequest(
                "context input must be a string or array".to_owned(),
            ));
        }
    };
    if incoming.iter().any(checkpoint::is_checkpoint_item) {
        let secret = crate::config::ensure_compaction_secret().map_err(GatewayError::Other)?;
        if let Some(expanded) = engine
            .expand_checkpoint(session, &incoming, &secret)
            .map_err(|error| GatewayError::BadRequest(error.to_string()))?
        {
            return Ok(expanded);
        }
    }
    match body.get("previous_response_id").and_then(Value::as_str) {
        Some(previous) => engine
            .expand_chained_input(session, previous, &incoming)
            .map_err(|error| GatewayError::BadRequest(error.to_string())),
        None => Ok(incoming),
    }
}

/// Session identity is the authenticated client plus the explicit thread or
/// session the client sent. Subagent routing hashes are included so a subagent
/// never shares the parent view.
pub(crate) fn context_session(
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
        let body = compose_body(&template, &base, &[], &engine, true, "view at 10%");
        let input = body["input"].as_array().expect("input array");
        assert_eq!(input[0], user("compressed view"));
        assert_eq!(input.len(), 2);
        assert!(
            input[1].to_string().contains("<context_state>"),
            "the pressure note rides at the end of the input"
        );
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
        assert!(
            !body["instructions"]
                .as_str()
                .unwrap_or_default()
                .contains("view at 10%"),
            "the instructions stay stable so the prefix cache survives each round"
        );
    }

    #[test]
    fn replaces_client_tools_that_shadow_internal_names() {
        let engine = engine();
        let template = json!({"tools": [{"type": "function", "name": "mixin_context_status"}]});
        let body = compose_body(&template, &[], &[], &engine, true, "view at 10%");
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 4);
    }

    #[test]
    fn guidance_only_keeps_the_guidance_without_internal_tools() {
        let engine = engine();
        let template = json!({"tools": [{"type": "function", "name": "shell"}]});
        let body = compose_body(&template, &[], &[], &engine, false, "view at 10%");
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "shell");
        assert!(
            body["instructions"]
                .as_str()
                .unwrap_or_default()
                .contains("<context_management>")
        );
    }

    fn raw(event: &str, payload: &Value) -> Bytes {
        encode_raw_event(event, &payload.to_string())
    }

    fn decoded(bytes: Bytes) -> Value {
        let events = crate::protocol::sse::drain_events(&mut bytes.to_vec());
        serde_json::from_str(&events[0].data).expect("event JSON")
    }

    #[test]
    fn keeps_one_stream_across_rounds() {
        let template = json!({});
        let mut client = ContextEvents::default();
        let mut seen = Vec::new();
        for (round, expected_index) in [("first", 0), ("second", 1)] {
            client.begin_round();
            let created = crate::protocol::sse::SseEvent {
                event: Some("response.created".to_owned()),
                data: json!({"type":"response.created", "response":{"id":round}}).to_string(),
            };
            let event = client.render(&created, &template).expect("created event");
            if round == "first" {
                seen.push(decoded(event.expect("first created")));
            } else {
                assert!(event.is_none(), "only one created event is visible");
            }
            let added = crate::protocol::sse::SseEvent {
                event: Some("response.output_item.added".to_owned()),
                data: json!({"response_id":round, "output_index":0, "item":{"type":"message", "id":round}}).to_string(),
            };
            let event = decoded(
                client
                    .render(&added, &template)
                    .expect("added event")
                    .expect("visible message"),
            );
            assert_eq!(event["response_id"], "first");
            assert_eq!(event["output_index"], expected_index);
            seen.push(event);
        }
        for (sequence, event) in seen.iter().enumerate() {
            assert_eq!(event["sequence_number"], sequence);
        }
    }

    #[test]
    fn holds_client_actions_until_commit() {
        let mut client = ContextEvents::default();
        let event = crate::protocol::sse::SseEvent {
            event: Some("response.output_item.added".to_owned()),
            data: json!({"item":{"type":"function_call", "id":"tool", "call_id":"call", "name":"shell"}}).to_string(),
        };
        assert!(
            client
                .render(&event, &json!({}))
                .expect("tool event")
                .is_none()
        );
        let delta = crate::protocol::sse::SseEvent {
            event: Some("response.function_call_arguments.delta".to_owned()),
            data: json!({"item_id":"tool", "delta":"{}"}).to_string(),
        };
        assert!(
            client
                .render(&delta, &json!({}))
                .expect("arguments event")
                .is_none()
        );
        let events = client.drain_actions();
        assert_eq!(events.len(), 2);
        assert_eq!(decoded(events[0].clone())["item"]["name"], "shell");
    }

    #[test]
    fn bounds_held_client_tool_events() {
        let mut client = ContextEvents::default();
        let event = crate::protocol::sse::SseEvent {
            event: Some("response.output_item.added".to_owned()),
            data: json!({"item":{"type":"function_call", "name":"shell", "arguments":"x".repeat(MAX_PENDING_TOOL_BYTES)}}).to_string(),
        };
        let error = client
            .render(&event, &json!({}))
            .expect_err("bounded tool events");
        assert!(error.to_string().contains("buffer limit"));
        assert!(client.pending_actions.is_empty());
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
        let bytes = ContextEvents::default().failure(&json!({"model": "test"}), "boom".to_owned());
        let text = String::from_utf8(bytes.to_vec()).expect("utf8 event");
        assert!(text.contains("event: response.failed"));
        assert!(text.contains("\"message\":\"boom\""));
    }
}
