use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, oneshot, watch};

type PendingResponse = oneshot::Sender<anyhow::Result<Value>>;

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
}

pub(crate) struct DucxAppServer {
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, PendingResponse>>>,
    events: broadcast::Sender<Value>,
    shutdown: watch::Sender<bool>,
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
}
