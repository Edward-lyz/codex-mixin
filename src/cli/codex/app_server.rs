use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::time::Duration;

use anyhow::Context;
use serde_json::Value;

pub(in crate::cli) struct AppServerReply {
    pub(in crate::cli) initialize: Value,
    pub(in crate::cli) result: Value,
}

pub(in crate::cli) fn request_app_server(
    cli: &Path,
    codex_home: &Path,
    method: &str,
    params: Option<Value>,
    timeout: Duration,
    client_name: &str,
) -> anyhow::Result<AppServerReply> {
    let mut child = ProcessCommand::new(cli)
        .arg("app-server")
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("start Codex app-server with {}", cli.display()))?;
    let pid = child.id();
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if cancel_rx.recv_timeout(timeout).is_err() {
            let _ = ProcessCommand::new("kill")
                .args(["-9", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    });
    let reply = exchange_app_server(&mut child, method, params, client_name);
    let _ = cancel_tx.send(());
    let _ = child.kill();
    let _ = child.wait();
    let _ = watchdog.join();
    reply
}

fn exchange_app_server(
    child: &mut Child,
    method: &str,
    mut params: Option<Value>,
    client_name: &str,
) -> anyhow::Result<AppServerReply> {
    let mut stdin = child
        .stdin
        .take()
        .context("Codex app-server stdin unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("Codex app-server stdout unavailable")?;
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {"name": client_name, "version": env!("CARGO_PKG_VERSION")}
            }
        })
    )?;
    stdin.flush()?;

    let mut initialize = None;
    for line in BufReader::new(stdout).lines() {
        let line = line.context("read Codex app-server response")?;
        let value: Value = serde_json::from_str(&line)
            .with_context(|| format!("parse Codex app-server response: {line}"))?;
        match value.get("id").and_then(Value::as_u64) {
            Some(1) => {
                let result = app_server_result(value, "initialize")?;
                initialize = Some(result);
                writeln!(stdin, "{}", serde_json::json!({"method": "initialized"}))?;
                let mut request = serde_json::json!({"id": 2, "method": method});
                if let Some(params) = params.take() {
                    request["params"] = params;
                }
                writeln!(stdin, "{request}")?;
                stdin.flush()?;
            }
            Some(2) => {
                return Ok(AppServerReply {
                    initialize: initialize
                        .context("Codex app-server skipped initialize response")?,
                    result: app_server_result(value, method)?,
                });
            }
            _ => {}
        }
    }
    anyhow::bail!("Codex app-server ended before answering {method}")
}

fn app_server_result(response: Value, method: &str) -> anyhow::Result<Value> {
    if let Some(result) = response.get("result") {
        return Ok(result.clone());
    }
    let error = response
        .get("error")
        .map(Value::to_string)
        .unwrap_or_else(|| "missing result and error".to_owned());
    anyhow::bail!("Codex app-server {method} failed: {error}")
}
