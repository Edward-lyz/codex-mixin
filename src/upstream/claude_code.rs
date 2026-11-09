//! Claude Code subscription transport.
//!
//! Requests run through the signed-in official `claude` CLI using the built-in
//! PTY/ConPTY adapter, with no print-mode fallback or external gateway.
//! The CLI owns authentication, its system prompt and its
//! request identity. The caller's tools reach the CLI as one loopback MCP
//! server. Each client thread keeps one CLI process: a tool call ends the
//! outer response while the CLI waits inside the MCP call, and the next
//! request's tool results answer that call so the same process continues.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::platform::claude_interactive::{InteractiveChild, InteractiveInput};
use anyhow::Context;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bytes::Bytes;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};

use super::{AnthropicByteStream, UpstreamAccess};
use crate::anthropic::{ContentBlock, Message, MessageRequest};
use crate::error::GatewayError;
use crate::provider::ProviderRuntime;

const CLI_NAME: &str = "claude";
const MCP_SERVER: &str = "mixin";
const MCP_PREFIX: &str = "mcp__mixin__";
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Tool names the Messages API accepts, prefix included.
const MAX_TOOL_NAME: usize = 64;
const MAX_IDLE_SESSIONS: usize = 4;
const IDLE_TTL: Duration = Duration::from_secs(20 * 60);
/// How long an MCP call waits for its tool_use block to be parsed when it
/// arrives without Claude Code's tool-use id.
const ANNOUNCE_WAIT: Duration = Duration::from_secs(5);
const EVENT_QUEUE: usize = 512;
/// The CLI waits on the outer agent's tool execution, which includes user
/// approval; its default MCP timeout would abort long commands.
const MCP_TOOL_TIMEOUT_MS: &str = "86400000";
/// Codex already truncates tool output; keep Claude Code from truncating it
/// again at its lower default.
const MAX_MCP_OUTPUT_TOKENS: &str = "100000";
/// Variables an enclosing Claude Code session sets for its children; the
/// CLI would otherwise treat itself as nested or reuse the parent's channel.
/// API-key variables would switch it from the subscription to API billing.
const INHERITED_ENV: [&str; 9] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
];
const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const HISTORY_INTRO: &str = "The conversation so far, recorded by the client application. Continue it; the tools named there are available to you.";
const CONTINUE_PROMPT: &str = "Continue from where the conversation above left off.";

/// Tool desks of the running processes, keyed by their MCP URL token.
type Desks = Arc<Mutex<HashMap<String, Arc<ToolDesk>>>>;

/// Live CLI processes keyed by client thread, plus the shared MCP bridge.
#[derive(Default)]
pub(crate) struct ClaudeCodeSessions {
    idle: tokio::sync::Mutex<HashMap<String, Session>>,
    bridge: tokio::sync::OnceCell<Bridge>,
}

#[derive(Clone)]
struct Bridge {
    port: u16,
    desks: Desks,
}

/// Tools one CLI process sees, and the rendezvous between its MCP calls and
/// the tool results that arrive with the client's next request.
#[derive(Default)]
struct ToolDesk {
    tools: Vec<Value>,
    client_names: HashMap<String, String>,
    state: Mutex<DeskState>,
    announced: Notify,
}

#[derive(Default)]
struct DeskState {
    /// tool_use blocks the model emitted, in order: (id, MCP name, input).
    announced: VecDeque<(String, String, Value)>,
    /// Ids the CLI is or will be waiting on.
    pending: Vec<String>,
    results: HashMap<String, Value>,
    waiters: HashMap<String, oneshot::Sender<Value>>,
}

struct Session {
    _child: InteractiveChild,
    stdin: InteractiveInput,
    events: mpsc::Receiver<Value>,
    desk: Arc<ToolDesk>,
    token: String,
    desks: Desks,
    fingerprint: u64,
    seen_messages: usize,
    last_used: Instant,
}

impl Drop for Session {
    fn drop(&mut self) {
        // Drop result senders too: a disconnected CLI must not leave MCP
        // requests holding this desk alive indefinitely.
        if let Ok(mut state) = self.desk.state.lock() {
            state.waiters.clear();
            state.results.clear();
        }
        if let Ok(mut desks) = self.desks.lock() {
            desks.remove(&self.token);
        }
    }
}

enum Step {
    /// Answer the waiting MCP calls, then optionally queue new user input.
    Resume(Vec<(String, Value)>, Option<Vec<Value>>),
    NewTurn(Vec<Value>),
}

impl UpstreamAccess {
    pub(crate) async fn send_claude_code(
        &self,
        provider: &ProviderRuntime,
        request: &MessageRequest,
        thread: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let sessions = self.claude_code.clone();
        let bridge = sessions.bridge().await?;
        let fingerprint = request_fingerprint(request);
        let reused = match thread {
            Some(thread) => sessions.take(thread).await,
            None => None,
        };
        let planned = reused.and_then(|session| {
            let step = plan_step(&session, request, fingerprint)?;
            Some((session, step))
        });
        tracing::debug!(
            provider_id = provider.id(),
            model = %request.model,
            reused = planned.is_some(),
            "interactive Claude Code request"
        );
        let (mut session, step) = match planned {
            Some(planned) => planned,
            None => {
                let session = spawn_session(provider, request, &bridge, fingerprint).await?;
                let opening = opening_message(&request.messages);
                (session, Step::NewTurn(opening))
            }
        };
        match step {
            Step::Resume(results, extra) => {
                session.desk.deliver(results);
                if let Some(content) = extra {
                    write_user(&mut session.stdin, content).await?;
                }
            }
            Step::NewTurn(content) => write_user(&mut session.stdin, content).await?,
        }
        session.seen_messages = request.messages.len();
        let thread = thread.map(str::to_owned);
        Ok(Box::pin(async_stream::stream! {
            let mut stop_reason = None::<String>;
            let mut started = false;
            loop {
                let Some(mut event) = session.events.recv().await else {
                    yield Ok(error_event("Claude Code exited before finishing the response"));
                    return;
                };
                if !event.get("parent_tool_use_id").is_none_or(Value::is_null) {
                    continue;
                }
                match event.get("type").and_then(Value::as_str) {
                    Some("stream_event") => {
                        let mut inner = event["event"].take();
                        let kind = inner.get("type").and_then(Value::as_str).unwrap_or_default().to_owned();
                        match kind.as_str() {
                            "message_start" => started = true,
                            "content_block_start" => session.desk.rename_block(&mut inner),
                            "message_delta" => {
                                stop_reason = inner["delta"]["stop_reason"].as_str().map(str::to_owned);
                            }
                            _ => {}
                        }
                        let encoded = crate::protocol::sse::encode_raw_event(&kind, &inner.to_string());
                        if kind != "message_stop" {
                            yield Ok(encoded);
                            continue;
                        }
                        // Consumers stop reading at message_stop, so the
                        // process is parked before that event goes out. On a
                        // tool call the CLI now blocks in the MCP calls and
                        // the next request carries their results; otherwise
                        // its turn ends with a result event first.
                        let healthy = stop_reason.as_deref() == Some("tool_use")
                            || turn_succeeded(&mut session.events).await;
                        if healthy && let Some(thread) = thread {
                            sessions.put(thread, session).await;
                        }
                        yield Ok(encoded);
                        return;
                    }
                    Some("assistant") if !started => {
                        if let Some(error) = event.get("error").and_then(Value::as_str) {
                            let text = assistant_text(&event).unwrap_or_else(|| error.to_owned());
                            yield Ok(error_event(&format!("Claude Code request failed: {text}")));
                            return;
                        }
                    }
                    Some("result") => {
                        let text = event["result"].as_str().unwrap_or("no response").to_owned();
                        yield Ok(error_event(&format!("Claude Code request failed: {text}")));
                        return;
                    }
                    _ => {}
                }
            }
        }))
    }
}

impl ClaudeCodeSessions {
    async fn bridge(&self) -> Result<Bridge, GatewayError> {
        let bridge = self
            .bridge
            .get_or_try_init(|| async {
                let desks: Desks = Arc::default();
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .context("bind the Claude Code tool bridge")?;
                let port = listener.local_addr()?.port();
                let app = Router::new()
                    .route("/mcp/{token}", post(mcp_call))
                    .with_state(desks.clone());
                tokio::spawn(async move {
                    if let Err(error) = axum::serve(listener, app).await {
                        tracing::error!(error = %error, "Claude Code tool bridge stopped");
                    }
                });
                Ok::<_, anyhow::Error>(Bridge { port, desks })
            })
            .await?;
        Ok(bridge.clone())
    }

    async fn take(&self, thread: &str) -> Option<Session> {
        self.idle.lock().await.remove(thread)
    }

    async fn put(&self, thread: String, mut session: Session) {
        session.last_used = Instant::now();
        let mut idle = self.idle.lock().await;
        idle.retain(|_, session| session.last_used.elapsed() < IDLE_TTL);
        while idle.len() >= MAX_IDLE_SESSIONS {
            let Some(oldest) = idle
                .iter()
                .min_by_key(|(_, session)| session.last_used)
                .map(|(thread, _)| thread.clone())
            else {
                break;
            };
            idle.remove(&oldest);
        }
        idle.insert(thread, session);
    }
}

impl ToolDesk {
    fn new(tools: &[Value]) -> Self {
        let mut desk = Self::default();
        for tool in tools {
            // Server tools (web search and similar) have no client executor.
            if tool
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind != "custom")
            {
                continue;
            }
            let Some(name) = tool.get("name").and_then(Value::as_str) else {
                continue;
            };
            let mcp_name = mcp_tool_name(name);
            desk.client_names.insert(mcp_name.clone(), name.to_owned());
            desk.tools.push(json!({
                "name": mcp_name,
                "description": tool.get("description").cloned().unwrap_or_else(|| json!("")),
                "inputSchema": tool
                    .get("input_schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"})),
            }));
        }
        desk
    }

    fn rename_block(&self, event: &mut Value) {
        let block = &mut event["content_block"];
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            return;
        }
        let Some(name) = block.get("name").and_then(Value::as_str) else {
            return;
        };
        let short = name.strip_prefix(MCP_PREFIX).unwrap_or(name);
        if let Some(client) = self.client_names.get(short) {
            block["name"] = Value::String(client.clone());
        }
    }

    fn announce(&self, id: String, name: String, input: Value) {
        if let Ok(mut state) = self.state.lock() {
            state.pending.push(id.clone());
            state.announced.push_back((id, name, input));
        }
        self.announced.notify_waiters();
    }

    fn pending(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|state| state.pending.clone())
            .unwrap_or_default()
    }

    fn deliver(&self, results: Vec<(String, Value)>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        for (id, result) in results {
            state.pending.retain(|pending| pending != &id);
            match state.waiters.remove(&id) {
                Some(waiter) => {
                    let _ = waiter.send(result);
                }
                None => {
                    state.results.insert(id, result);
                }
            }
        }
    }

    /// Resolve which tool_use block an MCP call executes and wait for its
    /// result from the client.
    async fn call(&self, name: &str, arguments: &Value, use_id: Option<&str>) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + ANNOUNCE_WAIT;
        let id = loop {
            let notified = self.announced.notified();
            if let Ok(mut state) = self.state.lock() {
                let position =
                    state
                        .announced
                        .iter()
                        .position(|(id, announced, input)| match use_id {
                            Some(use_id) => id == use_id,
                            None => announced == name && input == arguments,
                        });
                if let Some(position) = position {
                    break state.announced.remove(position).map(|(id, _, _)| id)?;
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                // Without a matching block the id from the CLI is still the
                // best key for the client's result.
                break use_id?.to_owned();
            }
        };
        let receiver = {
            let mut state = self.state.lock().ok()?;
            if let Some(result) = state.results.remove(&id) {
                return Some(result);
            }
            let (sender, receiver) = oneshot::channel();
            state.waiters.insert(id, sender);
            receiver
        };
        receiver.await.ok()
    }
}

/// Decide how a live process continues with this request, or `None` when
/// the client history no longer matches it and a fresh process is needed.
fn plan_step(session: &Session, request: &MessageRequest, fingerprint: u64) -> Option<Step> {
    if session.fingerprint != fingerprint || request.messages.len() <= session.seen_messages {
        return None;
    }
    let trailing = request
        .messages
        .iter()
        .rev()
        .take_while(|message| message.role == "user")
        .collect::<Vec<_>>();
    let mut results = HashMap::new();
    let mut extra = Vec::new();
    for message in trailing.iter().rev() {
        for block in &message.content {
            match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    results.insert(tool_use_id.clone(), mcp_result(content));
                }
                ContentBlock::Text { .. } | ContentBlock::Image { .. } => {
                    extra.push(serde_json::to_value(block).ok()?);
                }
                _ => {}
            }
        }
    }
    let pending = session.desk.pending();
    if pending.is_empty() {
        return (results.is_empty() && !extra.is_empty()).then_some(Step::NewTurn(extra));
    }
    if !pending.iter().all(|id| results.contains_key(id)) {
        return None;
    }
    let results = pending
        .into_iter()
        .filter_map(|id| results.remove(&id).map(|result| (id, result)))
        .collect();
    Some(Step::Resume(results, (!extra.is_empty()).then_some(extra)))
}

async fn spawn_session(
    provider: &ProviderRuntime,
    request: &MessageRequest,
    bridge: &Bridge,
    fingerprint: u64,
) -> Result<Session, GatewayError> {
    let program = crate::platform::agent_cli::find_command(CLI_NAME)
        .context("Claude Code CLI `claude` was not found; install it and sign in")?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let desk = Arc::new(ToolDesk::new(&request.tools));
    let mcp_config = json!({
        "mcpServers": {
            MCP_SERVER: {
                "type": "http",
                "url": format!("http://127.0.0.1:{}/mcp/{token}", bridge.port),
            }
        }
    });
    let workdir = std::env::temp_dir().join("codex-mixin-claude-code");
    crate::platform::ensure_owner_only_dir(&workdir)
        .context("create the Claude Code working directory")?;
    let mut command = tokio::process::Command::from(crate::platform::launcher_command(&program));
    command
        .args([
            "--model",
            &request.model,
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            &mcp_config.to_string(),
            "--allowedTools",
            &format!("mcp__{MCP_SERVER}"),
            "--setting-sources",
            "",
            "--disable-slash-commands",
        ])
        .current_dir(&workdir)
        .env("MCP_TOOL_TIMEOUT", MCP_TOOL_TIMEOUT_MS)
        .env("MAX_MCP_OUTPUT_TOKENS", MAX_MCP_OUTPUT_TOKENS)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(system) = system_text(request) {
        command.args(["--append-system-prompt", &system]);
    }
    if let Some(effort) = request_effort(request) {
        command.args(["--effort", effort]);
    }
    for name in INHERITED_ENV {
        command.env_remove(name);
    }
    let api_key = provider.definition().auth.api_key.trim();
    if !api_key.is_empty() {
        command.env(TOKEN_ENV, api_key);
    }
    crate::platform::prepare_background_tokio_command(&mut command);
    let (child, stdin, mut source) = tokio::task::spawn_blocking(move || {
        crate::platform::claude_interactive::spawn(command.as_std())
    })
    .await
    .context("start interactive Claude worker")??;
    let (sender, events) = mpsc::channel(EVENT_QUEUE);
    let tool_desk = desk.clone();
    tokio::spawn(async move {
        let mut blocks = HashMap::new();
        while let Some(event) = source.recv().await {
            if event["type"] == "rate_limits" {
                let headers = event["headers"].clone();
                let saved = tokio::task::spawn_blocking(move || {
                    crate::provider::claude_code::record_limits(&headers)
                })
                .await;
                if let Err(error) = saved.map_err(anyhow::Error::from).and_then(|result| result) {
                    tracing::warn!(error = %error, "could not record the Claude Code allowance");
                }
                continue;
            }
            if event["type"] == "stream_event" {
                track_tool_block(&event["event"], &mut blocks, &tool_desk);
            }
            if sender.send(event).await.is_err() {
                break;
            }
        }
    });
    if let Ok(mut desks) = bridge.desks.lock() {
        desks.insert(token.clone(), desk.clone());
    }
    Ok(Session {
        _child: child,
        stdin,
        events,
        desk,
        token,
        desks: bridge.desks.clone(),
        fingerprint,
        seen_messages: 0,
        last_used: Instant::now(),
    })
}

fn track_tool_block(
    event: &Value,
    blocks: &mut HashMap<u64, (String, String, String)>,
    desk: &ToolDesk,
) {
    let index = event["index"].as_u64().unwrap_or_default();
    match event["type"].as_str() {
        Some("content_block_start") if event["content_block"]["type"] == "tool_use" => {
            let block = &event["content_block"];
            let name = block["name"].as_str().unwrap_or_default();
            blocks.insert(
                index,
                (
                    block["id"].as_str().unwrap_or_default().to_owned(),
                    name.strip_prefix(MCP_PREFIX).unwrap_or(name).to_owned(),
                    String::new(),
                ),
            );
        }
        Some("content_block_delta") => {
            if let Some((_, _, input)) = blocks.get_mut(&index)
                && let Some(part) = event["delta"]["partial_json"].as_str()
            {
                input.push_str(part);
            }
        }
        Some("content_block_stop") => {
            if let Some((id, name, input)) = blocks.remove(&index) {
                let input = if input.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&input).unwrap_or(Value::Null)
                };
                desk.announce(id, name, input);
            }
        }
        _ => {}
    }
}

async fn mcp_call(
    State(desks): State<Desks>,
    Path(token): Path<String>,
    _headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response {
    let desk = desks
        .lock()
        .ok()
        .and_then(|desks| desks.get(&token).cloned());
    let Some(desk) = desk else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(id) = message.get("id").cloned() else {
        // Notifications need no reply.
        return StatusCode::ACCEPTED.into_response();
    };
    let params = &message["params"];
    let result = match message["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(MCP_PROTOCOL_VERSION),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "codex-mixin", "version": env!("CARGO_PKG_VERSION")},
        }),
        Some("ping") => json!({}),
        Some("tools/list") => json!({"tools": desk.tools}),
        Some("tools/call") => {
            let name = params["name"].as_str().unwrap_or_default();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let use_id = params["_meta"]["claudecode/toolUseId"].as_str();
            match desk.call(name, &arguments, use_id).await {
                Some(result) => result,
                None => {
                    return Json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32000, "message": "the client did not return this tool result"},
                    }))
                    .into_response();
                }
            }
        }
        _ => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "method not found"},
            }))
            .into_response();
        }
    };
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

async fn write_user(stdin: &mut InteractiveInput, content: Vec<Value>) -> Result<(), GatewayError> {
    stdin
        .send(content)
        .await
        .context("send the message to interactive Claude")?;
    Ok(())
}

/// First message of a fresh process: earlier history as a transcript, then
/// the latest user input as real content blocks.
fn opening_message(messages: &[Message]) -> Vec<Value> {
    let last_is_plain_user = messages.last().is_some_and(|message| {
        message.role == "user"
            && !message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    });
    let (history, latest) = match messages.split_last() {
        Some((last, history)) if last_is_plain_user => (history, Some(last)),
        _ => (messages, None),
    };
    let mut content = Vec::new();
    if !history.is_empty() {
        let mut transcript = format!("{HISTORY_INTRO}\n<history>\n");
        for message in history {
            render_message(message, &mut transcript);
        }
        transcript.push_str("</history>");
        content.push(json!({"type": "text", "text": transcript}));
    }
    match latest {
        Some(latest) => content.extend(
            latest
                .content
                .iter()
                .filter(|block| {
                    matches!(
                        block,
                        ContentBlock::Text { .. } | ContentBlock::Image { .. }
                    )
                })
                .filter_map(|block| serde_json::to_value(block).ok()),
        ),
        None => content.push(json!({"type": "text", "text": CONTINUE_PROMPT})),
    }
    content
}

fn render_message(message: &Message, out: &mut String) {
    for block in &message.content {
        let line = match block {
            ContentBlock::Text { text } => format!("[{}] {text}", message.role),
            ContentBlock::Image { .. } => format!("[{}] (image)", message.role),
            ContentBlock::ToolUse { id, name, input } => {
                format!("[assistant tool call {id}] {name} {input}")
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => format!("[tool result {tool_use_id}] {}", result_text(content)),
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => continue,
        };
        out.push_str(&line);
        out.push('\n');
    }
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| match part["type"].as_str() {
                Some("text") => part["text"].as_str().unwrap_or_default().to_owned(),
                Some("image") => "(image)".to_owned(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

fn mcp_result(content: &Value) -> Value {
    let parts = match content {
        Value::Array(parts) => parts
            .iter()
            .map(|part| match part["type"].as_str() {
                Some("image") if part["source"]["type"] == "base64" => json!({
                    "type": "image",
                    "data": part["source"]["data"],
                    "mimeType": part["source"]["media_type"],
                }),
                Some("text") => json!({"type": "text", "text": part["text"]}),
                _ => {
                    json!({"type": "text", "text": result_text(&Value::Array(vec![part.clone()]))})
                }
            })
            .collect(),
        other => vec![json!({"type": "text", "text": result_text(other)})],
    };
    json!({"content": parts, "isError": false})
}

fn mcp_tool_name(name: &str) -> String {
    let limit = MAX_TOOL_NAME - MCP_PREFIX.len();
    if name.len() <= limit {
        return name.to_owned();
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, name.as_bytes());
    let suffix = digest.as_ref()[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let keep = name
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index < limit - suffix.len())
        .last()
        .unwrap_or_default();
    format!("{}_{suffix}", &name[..keep])
}

fn system_text(request: &MessageRequest) -> Option<String> {
    let text = request
        .system
        .iter()
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.trim().is_empty()).then_some(text)
}

fn request_effort(request: &MessageRequest) -> Option<&'static str> {
    let effort = request.output_config.as_ref()?.get("effort")?.as_str()?;
    EFFORT_LEVELS.into_iter().find(|level| *level == effort)
}

/// What a running process was started with; any change needs a new one.
fn request_fingerprint(request: &MessageRequest) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    request.model.hash(&mut hasher);
    system_text(request).hash(&mut hasher);
    request_effort(request).hash(&mut hasher);
    for tool in &request.tools {
        tool.to_string().hash(&mut hasher);
    }
    hasher.finish()
}

/// Read the rest of a finished turn up to its result event.
async fn turn_succeeded(events: &mut mpsc::Receiver<Value>) -> bool {
    while let Some(event) = events.recv().await {
        if event.get("type").and_then(Value::as_str) == Some("result")
            && event.get("parent_tool_use_id").is_none_or(Value::is_null)
        {
            return event.get("is_error").and_then(Value::as_bool) != Some(true);
        }
    }
    false
}

fn assistant_text(event: &Value) -> Option<String> {
    event["message"]["content"]
        .as_array()?
        .iter()
        .find_map(|block| block["text"].as_str().map(str::to_owned))
}

fn error_event(message: &str) -> Bytes {
    let payload = json!({"type": "error", "error": {"type": "api_error", "message": message}});
    crate::protocol::sse::encode_raw_event("error", &payload.to_string())
}
