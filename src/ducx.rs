use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, oneshot, watch};

type PendingResponse = oneshot::Sender<anyhow::Result<Value>>;

pub(crate) fn default_ducx_executable() -> Option<PathBuf> {
    let installed = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".baidu-cx/baidu-cx/bin/ducx"))
        .filter(|path| path.is_file());
    installed.or_else(|| {
        env::var_os("PATH").and_then(|paths| {
            env::split_paths(&paths)
                .map(|path| path.join("ducx"))
                .find(|path| path.is_file())
        })
    })
}

#[derive(Clone, Debug)]
pub(crate) struct DucxProcessConfig {
    pub(crate) executable: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: Vec<(String, String)>,
}

impl DucxProcessConfig {
    pub(crate) fn new(executable: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: Vec::new(),
        }
    }

    pub(crate) fn app_server(executable: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        let disabled_features = [
            "apps",
            "browser_use",
            "browser_use_external",
            "browser_use_full_cdp_access",
            "code_mode_host",
            "computer_use",
            "goals",
            "hooks",
            "image_generation",
            "in_app_browser",
            "multi_agent",
            "plugin_sharing",
            "plugins",
            "remote_plugin",
            "shell_tool",
            "skill_mcp_dependency_install",
            "skill_search",
            "tool_call_mcp_elicitation",
            "tool_suggest",
            "unified_exec",
            "workspace_dependencies",
        ];
        let mut args = disabled_features
            .into_iter()
            .flat_map(|feature| ["--disable".to_owned(), feature.to_owned()])
            .collect::<Vec<_>>();
        args.extend([
            "app-server".to_owned(),
            "--listen".to_owned(),
            "stdio://".to_owned(),
            "-c".to_owned(),
            "history.persistence=\"none\"".to_owned(),
            "-c".to_owned(),
            "analytics.enabled=false".to_owned(),
            "-c".to_owned(),
            "feedback.enabled=false".to_owned(),
            "-c".to_owned(),
            "web_search=\"disabled\"".to_owned(),
            "-c".to_owned(),
            "project_doc_max_bytes=0".to_owned(),
            "-c".to_owned(),
            "project_doc_fallback_filenames=[]".to_owned(),
        ]);
        Self {
            executable: executable.into(),
            args,
            cwd: cwd.into(),
            env: Vec::new(),
        }
    }
}

pub(crate) struct DucxAppServer {
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, PendingResponse>>>,
    events: broadcast::Sender<Value>,
    shutdown: watch::Sender<bool>,
}

pub(crate) struct DucxTurn {
    thread_id: String,
    turn_id: String,
    events: broadcast::Receiver<Value>,
}

pub(crate) fn build_turn_params(
    request: &Value,
    upstream_model: &str,
    cwd: &Path,
) -> anyhow::Result<(Value, Value)> {
    let instructions = request
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let dynamic_tools = map_dynamic_tools(request.get("tools"))?;
    let input = map_turn_input(
        request
            .get("input")
            .context("Responses request is missing input")?,
    )?;
    let cwd = cwd
        .to_str()
        .context("DUCX working directory is not valid UTF-8")?;
    let thread = json!({
        "approvalPolicy": "never",
        "baseInstructions": instructions,
        "developerInstructions": "",
        "dynamicTools": dynamic_tools,
        "environments": [],
        "ephemeral": true,
        "experimentalRawEvents": true,
        "cwd": cwd,
        "model": upstream_model,
        "modelProvider": "oneapi",
        "runtimeWorkspaceRoots": [],
        "sandbox": "read-only",
        "config": {
            "mcp_servers": {}
        }
    });
    let turn = json!({
        "input": [{"type":"text","text":input}],
        "approvalPolicy": "never",
        "cwd": cwd,
        "environments": [],
        "runtimeWorkspaceRoots": [],
        "sandboxPolicy": {
            "type": "readOnly",
            "networkAccess": false
        }
    });
    Ok((thread, turn))
}

fn map_dynamic_tools(tools: Option<&Value>) -> anyhow::Result<Vec<Value>> {
    let Some(tools) = tools else {
        return Ok(Vec::new());
    };
    let tools = tools
        .as_array()
        .context("Responses tools must be an array")?;
    tools
        .iter()
        .map(|tool| {
            ensure!(
                tool.get("type").and_then(Value::as_str) == Some("function"),
                "DUCX app-server currently supports only function tools"
            );
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .context("Responses function tool is missing name")?;
            let input_schema = tool
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            Ok(json!({
                "type": "function",
                "name": name,
                "description": tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "inputSchema": input_schema
            }))
        })
        .collect()
}

fn map_turn_input(input: &Value) -> anyhow::Result<String> {
    if let Some(text) = input.as_str() {
        ensure!(!text.is_empty(), "Responses input must not be empty");
        return Ok(text.to_owned());
    }
    let items = input
        .as_array()
        .context("DUCX app-server currently supports string or message-array input")?;
    let mut transcript = Vec::new();
    for item in items {
        ensure!(
            item.get("type").and_then(Value::as_str) == Some("message"),
            "DUCX app-server currently supports only message input items"
        );
        let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = item
            .get("content")
            .context("Responses message input is missing content")?;
        let text = if let Some(text) = content.as_str() {
            text.to_owned()
        } else {
            let parts = content
                .as_array()
                .context("Responses message content must be text or an array")?;
            let mut text = Vec::new();
            for part in parts {
                let part_type = part.get("type").and_then(Value::as_str);
                ensure!(
                    matches!(part_type, Some("input_text" | "text")),
                    "DUCX app-server currently supports only text message content"
                );
                text.push(
                    part.get("text")
                        .and_then(Value::as_str)
                        .context("Responses text content is missing text")?,
                );
            }
            text.join("\n")
        };
        transcript.push(format!("[{role}]\n{text}"));
    }
    ensure!(!transcript.is_empty(), "Responses input must not be empty");
    Ok(transcript.join("\n\n"))
}

impl DucxAppServer {
    pub(crate) async fn spawn(config: DucxProcessConfig) -> anyhow::Result<Self> {
        ensure!(
            config.executable.is_file(),
            "DUCX executable does not exist: {}",
            config.executable.display()
        );
        ensure!(
            config.cwd.is_dir(),
            "DUCX working directory does not exist: {}",
            config.cwd.display()
        );

        let mut child = Command::new(&config.executable)
            .args(&config.args)
            .current_dir(&config.cwd)
            .envs(config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "start DUCX app-server executable {}",
                    config.executable.display()
                )
            })?;
        let stdin = child.stdin.take().context("capture DUCX stdin")?;
        let stdout = child.stdout.take().context("capture DUCX stdout")?;
        let stderr = child.stderr.take().context("capture DUCX stderr")?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(512);
        let (shutdown, mut shutdown_rx) = watch::channel(false);

        let reader_pending = Arc::clone(&pending);
        let reader_events = events.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        handle_stdout_line(&line, &reader_pending, &reader_events).await;
                    }
                    Ok(None) => {
                        fail_pending(&reader_pending, "DUCX app-server stdout closed").await;
                        break;
                    }
                    Err(error) => {
                        fail_pending(
                            &reader_pending,
                            &format!("read DUCX app-server stdout: {error}"),
                        )
                        .await;
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(_)) = lines.next_line().await {
                tracing::debug!("DUCX app-server emitted a stderr line");
            }
        });

        let waiter_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => {
                    let message = match status {
                        Ok(status) => format!("DUCX app-server exited with {status}"),
                        Err(error) => format!("wait for DUCX app-server: {error}"),
                    };
                    fail_pending(&waiter_pending, &message).await;
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_ok() && *shutdown_rx.borrow() {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        fail_pending(&waiter_pending, "DUCX app-server stopped").await;
                    }
                }
            }
        });

        Ok(Self {
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending,
            events,
            shutdown,
        })
    }

    pub(crate) async fn spawn_ready(
        config: DucxProcessConfig,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let cwd = config.cwd.clone();
        let server = Self::spawn(config).await?;
        server
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "codex_mixin",
                        "title": "Codex Mixin",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {"experimentalApi": true}
                }),
                timeout,
            )
            .await
            .context("initialize DUCX app-server")?;
        server.notify("initialized", json!({})).await?;
        let hooks = server
            .request("hooks/list", json!({"cwds":[cwd]}), timeout)
            .await
            .context("audit DUCX hooks")?;
        let hook_count = hooks
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("hooks").and_then(Value::as_array))
            .map(Vec::len)
            .sum::<usize>();
        ensure!(
            hook_count == 0,
            "DUCX app-server exposed {hook_count} hooks after isolation"
        );
        Ok(server)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        self.write_json(&json!({ "method": method, "params": params }))
            .await
    }

    pub(crate) async fn respond(&self, id: Value, result: Value) -> anyhow::Result<()> {
        self.write_json(&json!({ "id": id, "result": result }))
            .await
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = self
            .write_json(&json!({ "id": id, "method": method, "params": params }))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => anyhow::bail!("DUCX response channel closed for {method}"),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                anyhow::bail!("DUCX request {method} timed out after {timeout:?}")
            }
        }
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    pub(crate) async fn start_turn(
        &self,
        thread_params: Value,
        turn_params: Value,
        timeout: Duration,
    ) -> anyhow::Result<DucxTurn> {
        let events = self.subscribe();
        let thread = self
            .request("thread/start", thread_params, timeout)
            .await
            .context("start DUCX thread")?;
        let thread_id = thread
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .context("DUCX thread/start response is missing thread.id")?
            .to_owned();
        let mut turn_params = turn_params;
        let params = turn_params
            .as_object_mut()
            .context("DUCX turn params must be an object")?;
        params.insert("threadId".to_owned(), Value::String(thread_id.clone()));
        let turn = self
            .request("turn/start", turn_params, timeout)
            .await
            .context("start DUCX turn")?;
        let turn_id = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .context("DUCX turn/start response is missing turn.id")?
            .to_owned();
        Ok(DucxTurn {
            thread_id,
            turn_id,
            events,
        })
    }

    async fn write_json(&self, message: &Value) -> anyhow::Result<()> {
        let mut encoded = serde_json::to_vec(message).context("encode DUCX JSON-RPC message")?;
        encoded.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&encoded)
            .await
            .context("write DUCX JSON-RPC message")?;
        stdin.flush().await.context("flush DUCX stdin")
    }
}

impl DucxTurn {
    pub(crate) fn into_stream(mut self) -> BoxStream<'static, anyhow::Result<Value>> {
        async_stream::try_stream! {
            loop {
                let message = self.events.recv().await.context("receive DUCX turn event")?;
                let params = message.get("params").unwrap_or(&Value::Null);
                if params.get("threadId").and_then(Value::as_str) != Some(&self.thread_id) {
                    continue;
                }
                if let Some(event_turn_id) = params.get("turnId").and_then(Value::as_str)
                    && event_turn_id != self.turn_id
                {
                    continue;
                }
                let completed = message.get("method").and_then(Value::as_str) == Some("turn/completed")
                    && params
                        .pointer("/turn/id")
                        .and_then(Value::as_str)
                        .is_none_or(|id| id == self.turn_id);
                yield message;
                if completed {
                    break;
                }
            }
        }
        .boxed()
    }
}

impl Drop for DucxAppServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn handle_stdout_line(
    line: &str,
    pending: &Mutex<HashMap<u64, PendingResponse>>,
    events: &broadcast::Sender<Value>,
) {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        tracing::warn!("DUCX app-server emitted non-JSON stdout");
        return;
    };
    if let Some(id) = message.get("id").and_then(Value::as_u64)
        && message.get("method").is_none()
        && let Some(sender) = pending.lock().await.remove(&id)
    {
        let response = match message.get("error") {
            Some(error) => Err(anyhow::anyhow!(
                "DUCX request failed: {}",
                redact_rpc_error(error)
            )),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = sender.send(response);
        return;
    }
    let _ = events.send(message);
}

async fn fail_pending(pending: &Mutex<HashMap<u64, PendingResponse>>, message: &str) {
    let senders = {
        let mut pending = pending.lock().await;
        pending
            .drain()
            .map(|(_, sender)| sender)
            .collect::<Vec<_>>()
    };
    for sender in senders {
        let _ = sender.send(Err(anyhow::anyhow!("{message}")));
    }
}

fn redact_rpc_error(error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown DUCX error");
    match code {
        Some(code) => format!("code {code}: {message}"),
        None => message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use serde_json::json;

    use super::*;

    fn mock_server() -> (tempfile::TempDir, DucxProcessConfig) {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("mock-ducx.cjs");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env node
const readline = require("node:readline");
const input = readline.createInterface({ input: process.stdin });
input.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
    process.stdout.write(JSON.stringify({ id: message.id, result: { ok: true } }) + "\n");
    process.stdout.write(JSON.stringify({ method: "initialized/event", params: { value: 7 } }) + "\n");
  } else if (message.method === "thread/start") {
    process.stdout.write(JSON.stringify({ id: message.id, result: { thread: { id: "thread_1" } } }) + "\n");
  } else if (message.method === "turn/start") {
    process.stdout.write(JSON.stringify({ id: message.id, result: { turn: { id: "turn_1" } } }) + "\n");
    process.stdout.write(JSON.stringify({ method: "item/agentMessage/delta", params: { threadId: "other", turnId: "turn_other", itemId: "x", delta: "ignore" } }) + "\n");
    process.stdout.write(JSON.stringify({ method: "item/agentMessage/delta", params: { threadId: message.params.threadId, turnId: "turn_1", itemId: "message_1", delta: "hello" } }) + "\n");
    process.stdout.write(JSON.stringify({ method: "turn/completed", params: { threadId: message.params.threadId, turn: { id: "turn_1", status: "completed" } } }) + "\n");
  } else if (message.method === "fail") {
    process.stdout.write(JSON.stringify({ id: message.id, error: { code: -1, message: "expected failure" } }) + "\n");
  }
});
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let config = DucxProcessConfig::new(&executable, directory.path());
        (directory, config)
    }

    #[tokio::test]
    async fn multiplexes_responses_and_notifications() {
        let (_directory, config) = mock_server();
        let server = DucxAppServer::spawn(config).await.unwrap();
        let mut events = server.subscribe();

        let response = server
            .request("initialize", json!({}), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(response, json!({ "ok": true }));
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["method"], "initialized/event");
        assert_eq!(event["params"]["value"], 7);
        server.shutdown();
    }

    #[tokio::test]
    async fn reports_rpc_errors_without_serializing_extra_fields() {
        let (_directory, config) = mock_server();
        let server = DucxAppServer::spawn(config).await.unwrap();

        let error = server
            .request("fail", json!({}), Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "DUCX request failed: code -1: expected failure"
        );
        server.shutdown();
    }

    #[tokio::test]
    async fn removes_timed_out_requests() {
        let (_directory, config) = mock_server();
        let server = DucxAppServer::spawn(config).await.unwrap();

        let error = server
            .request("never", json!({}), Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(server.pending.lock().await.is_empty());
        server.shutdown();
    }

    #[tokio::test]
    async fn isolates_concurrent_turn_notifications() {
        let (_directory, config) = mock_server();
        let server = DucxAppServer::spawn(config).await.unwrap();
        let turn = server
            .start_turn(json!({}), json!({ "input": [] }), Duration::from_secs(2))
            .await
            .unwrap();
        let events = turn
            .into_stream()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>()
            .await;

        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["params"]["delta"], "hello");
        assert_eq!(events[1]["method"], "turn/completed");
        server.shutdown();
    }

    #[test]
    fn maps_responses_instructions_tools_and_text() {
        let request = json!({
            "instructions": "SYSTEM",
            "input": "hello",
            "tools": [{
                "type": "function",
                "name": "lookup",
                "description": "Look something up",
                "parameters": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }
            }]
        });
        let (thread, turn) =
            build_turn_params(&request, "gpt-5.6-luna", Path::new("/tmp")).unwrap();

        assert_eq!(thread["baseInstructions"], "SYSTEM");
        assert_eq!(thread["developerInstructions"], "");
        assert_eq!(thread["modelProvider"], "oneapi");
        assert_eq!(thread["dynamicTools"][0]["name"], "lookup");
        assert_eq!(
            thread["dynamicTools"][0]["inputSchema"]["required"][0],
            "query"
        );
        assert_eq!(turn["input"][0]["text"], "hello");
    }

    #[test]
    fn rejects_non_text_input_without_silently_dropping_it() {
        let request = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type":"input_image","image_url":"https://example.test/a.png"}]
            }]
        });
        let error = build_turn_params(&request, "gpt-5.6-luna", Path::new("/tmp")).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("currently supports only text message content")
        );
    }

    #[test]
    fn rejects_builtin_tools_without_silently_dropping_them() {
        let request = json!({
            "input": "hello",
            "tools": [{"type":"web_search"}]
        });
        let error = build_turn_params(&request, "gpt-5.6-luna", Path::new("/tmp")).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("currently supports only function tools")
        );
    }
}
