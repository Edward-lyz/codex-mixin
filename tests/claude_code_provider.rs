//! The Claude Code provider drives the local `claude` CLI. A scripted CLI on
//! PATH runs inside a real PTY, writes captured SSE and calls the gateway's MCP
//! bridge, so the test covers discovery, the tool round trip and process
//! reuse through the real binary.
#![cfg(unix)]

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;

const MODEL: &str = "claude-sonnet-5-5-claude-code";

const FAKE_CLAUDE: &str = r#"#!/usr/bin/env node
const readline = require('readline');
const args = process.argv.slice(2);
if (args.includes('-p') || args.includes('--print')) throw Error('headless Claude must not run');
const url = JSON.parse(args[args.indexOf('--mcp-config') + 1]).mcpServers.mixin.url;
let turns = 0;
let rpc = 0;
const fs = require('fs');
if (!process.stdin.isTTY || !process.stdout.isTTY) throw Error('interactive terminal required');
const out = value => fs.appendFileSync(process.env.CODEX_MIXIN_CLAUDE_TAP, JSON.stringify(value) + '\n');
process.stdout.write('❯ ');
process.stdin.setRawMode(true);
const ev = (event) => out({ type: 'stream_event', event, parent_tool_use_id: null });
const mcp = async (method, params) => {
  const reply = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: ++rpc, method, params }),
  });
  return (await reply.json()).result;
};
function message(blocks, stop) {
  ev({ type: 'message_start', message: { id: `msg_${turns}_${stop}`, type: 'message', role: 'assistant', model: 'fake', content: [], stop_reason: null, usage: { input_tokens: 3, output_tokens: 0 } } });
  blocks.forEach((block, index) => {
    if (block.type === 'text') {
      ev({ type: 'content_block_start', index, content_block: { type: 'text', text: '' } });
      ev({ type: 'content_block_delta', index, delta: { type: 'text_delta', text: block.text } });
    } else {
      ev({ type: 'content_block_start', index, content_block: { type: 'tool_use', id: block.id, name: block.name, input: {} } });
      ev({ type: 'content_block_delta', index, delta: { type: 'input_json_delta', partial_json: JSON.stringify(block.input) } });
    }
    ev({ type: 'content_block_stop', index });
  });
  ev({ type: 'message_delta', delta: { stop_reason: stop }, usage: { output_tokens: 5 } });
  ev({ type: 'message_stop' });
}
out({ tap: 'on' });
const resetAt = Math.floor(Date.now() / 1000) + 3600;
const limits = { 'anthropic-ratelimit-unified-5h-utilization': '0.35', 'anthropic-ratelimit-unified-5h-reset': String(resetAt), 'anthropic-ratelimit-unified-7d-utilization': '0.05', 'anthropic-ratelimit-unified-7d-reset': String(resetAt + 86400) };
out({ type: 'system', subtype: 'init' });
const onInput = async text => {
  text = text.trim();
  turns += 1;
  out({ rate: limits });
  if (turns === 1) {
    await mcp('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'fake', version: '1' } });
    const name = (await mcp('tools/list', {})).tools[0].name;
    message([{ type: 'tool_use', id: 'toolu_fake_1', name: `mcp__mixin__${name}`, input: { cmd: 'echo hi' } }], 'tool_use');
    const result = await mcp('tools/call', { name, arguments: { cmd: 'echo hi' } });
    message([{ type: 'text', text: `tool said: ${result.content[0].text}` }], 'end_turn');
  } else {
    message([{ type: 'text', text: `process turn ${turns}: ${text}` }], 'end_turn');
  }
};
let buffer = '';
process.stdin.on('data', bytes => {
  process.stdout.write(bytes);
  buffer += bytes.toString();
  const end = buffer.indexOf('\x1b[201~\r');
  if (end < 0) return;
  const start = buffer.indexOf('\x1b[200~');
  const text = buffer.slice(start + 6, end);
  buffer = buffer.slice(end + 7);
  onInput(text).catch(() => process.exit(1));
});
"#;

struct Gateway(tokio::process::Child);

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .unwrap()
        .port()
}

fn install_fake_cli(root: &Path) -> std::path::PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cli = bin.join("claude");
    std::fs::write(&cli, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    // An interactive login leaves this credential file behind.
    let login = root.join("home/.claude/.credentials.json");
    std::fs::create_dir_all(login.parent().unwrap()).unwrap();
    std::fs::write(&login, r#"{"claudeAiOauth":{"refreshToken":"refresh"}}"#).unwrap();
    bin
}

async fn turn(address: SocketAddr, input: &[Value]) -> Vec<Value> {
    let body = json!({
        "model": MODEL,
        "stream": true,
        "instructions": "You are a coding agent.",
        "input": input,
        "tools": [{
            "type": "function",
            "name": "exec_command",
            "description": "Run a shell command",
            "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}},
        }],
    });
    let text = reqwest::Client::new()
        .post(format!("http://{address}/v1/responses"))
        .header("thread-id", "claude-code-thread")
        .json(&body)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .filter(|event| event["type"] == "response.output_item.done")
        .map(|event| event["item"].clone())
        .collect()
}

fn output_text(items: &[Value]) -> String {
    items
        .iter()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().cloned().unwrap_or_default())
        .filter_map(|part| part["text"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn claude_code_login_serves_tool_turns_on_one_cli_process() {
    let root = tempfile::tempdir().unwrap();
    let bin = install_fake_cli(root.path());
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let address: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let mut command = common::tokio_cli(root.path());
    command
        .env("PATH", path)
        .args(["serve", "--bind", &address.to_string(), "--no-client-sync"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let _gateway = Gateway(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    while TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err() {
        assert!(Instant::now() < deadline, "gateway did not listen");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut input = vec![json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": "run echo hi"}],
    })];
    let first = turn(address, &input).await;
    let call = first
        .iter()
        .find(|item| item["type"] == "function_call")
        .expect("the CLI's MCP tool call reaches the client");
    assert_eq!(call["name"], "exec_command");
    assert_eq!(call["call_id"], "toolu_fake_1");
    assert_eq!(
        serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
        json!({"cmd": "echo hi"})
    );

    input.extend([
        json!({"type": "function_call", "call_id": "toolu_fake_1", "name": "exec_command", "arguments": "{\"cmd\":\"echo hi\"}"}),
        json!({"type": "function_call_output", "call_id": "toolu_fake_1", "output": "hello-output"}),
    ]);
    let second = turn(address, &input).await;
    assert_eq!(output_text(&second), "tool said: hello-output");

    input.extend([
        json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "tool said: hello-output"}]}),
        json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "again"}]}),
    ]);
    let third = turn(address, &input).await;
    assert_eq!(output_text(&third), "process turn 2: again");
}

#[test]
fn claude_code_allowance_from_replies_reaches_quota() {
    // Reuses the gateway test's login layout; the allowance arrives with replies.
    let root = tempfile::tempdir().unwrap();
    let snapshot = root.path().join("oauth/claude-code-limits.json");
    std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
    let reset = chrono::Utc::now().timestamp() + 3600;
    std::fs::write(
        &snapshot,
        json!({"captured_at_ms": chrono::Utc::now().timestamp_millis(), "windows": {
            "five_hour": {"utilization": 0.35, "reset": reset},
            "seven_day": {"utilization": 0.05, "reset": reset + 86400}}})
        .to_string(),
    )
    .unwrap();
    let login = root.path().join("home/.claude/.credentials.json");
    std::fs::create_dir_all(login.parent().unwrap()).unwrap();
    std::fs::write(&login, r#"{"claudeAiOauth":{"refreshToken":"refresh"}}"#).unwrap();
    let bin = install_fake_cli(root.path());
    let output = common::cli(root.path())
        .env(
            "PATH",
            std::env::join_paths(
                std::iter::once(bin)
                    .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap(),
        )
        .args(["--no-tui", "quota", "--provider", "claude-code", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["quota_id"], "five_hour");
    assert_eq!(rows[0]["used"], 35.0);
    assert_eq!(rows[0]["remaining"], 65.0);
    assert_eq!(rows[1]["used"], 5.0);
}
