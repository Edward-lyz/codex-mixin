//! End-to-end coverage for the DUCX reporting hook model resolution.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::{ProviderModel, ProviderPreset};
use serde_json::{Value, json};

const SLUG: &str = "DeepSeek-V4.1-Flash-baidu-oneapi";

fn write_reporting_config(root: &Path) {
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu-oneapi", "fixture-key");
    provider.quota_username = Some("fixture-user".to_owned());
    provider.selected_models = vec!["DeepSeek-V4.1-Flash".to_owned()];
    provider.request_policy.baidu_code_report = true;
    provider.request_policy.data_report_client_token = Some("fixture-token".to_owned());
    save_stored_config_to_path(
        &root.join("config.json"),
        &StoredGatewayConfig {
            providers: vec![provider],
            ..Default::default()
        },
    )
    .expect("save reporting config");
}

/// Config for tests that start a real gateway and let it record the turn.
fn write_gateway_reporting_config(root: &Path, upstream: SocketAddr) {
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu-oneapi", "fixture-key");
    provider.base_url = format!("http://{upstream}");
    provider.quota_username = Some("fixture-user".to_owned());
    provider.cached_models = vec![ProviderModel {
        id: "DeepSeek-V4.1-Flash".to_owned(),
        context_window: Some(1_000_000),
        ..ProviderModel::default()
    }];
    provider.selected_models = vec!["DeepSeek-V4.1-Flash".to_owned()];
    provider.request_policy.baidu_code_report = true;
    provider.request_policy.data_report_client_token = Some("fixture-token".to_owned());
    provider.models_refreshed_at_ms = Some(1);
    let mut config = StoredGatewayConfig {
        providers: vec![provider],
        ..Default::default()
    };
    // Force the native Messages request through the gateway's converted plan
    // so the real usage observer records the turn.
    config.context.enabled = true;
    save_stored_config_to_path(&root.join("config.json"), &config)
        .expect("save gateway reporting config");
}

/// Minimal Anthropic SSE reply the gateway can persist as a usage turn.
fn anthropic_sse() -> String {
    [
        json!({"type":"message_start","message":{"id":"msg_usage","type":"message","role":"assistant","content":[],"model":"DeepSeek-V4.1-Flash","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ]
    .into_iter()
    .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
    .collect()
}

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut header_end = None;
    let mut content_length = 0;
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if header_end.is_none()
            && let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n")
        {
            header_end = Some(position + 4);
            for line in request[..position].split(|byte| *byte == b'\n') {
                let Ok(line) = std::str::from_utf8(line) else {
                    continue;
                };
                let Some((name, value)) = line.split_once(':') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
        }
        if let Some(end) = header_end
            && request.len() >= end + content_length
        {
            break;
        }
    }
    Ok(request)
}

/// Deterministic local upstream accepting the gateway's Anthropic requests.
fn spawn_mock_anthropic() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock upstream");
    let address = listener.local_addr().expect("mock upstream address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            std::thread::spawn(move || {
                // Read the complete request before replying; closing a socket
                // with unread body bytes makes the gateway see a reset.
                let _ = read_http_request(&mut stream);
                let body = anthropic_sse();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        }
    });
    address
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("reserve gateway port")
        .port()
}

fn wait_for_gateway(address: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "gateway did not listen on {address}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn send_gateway_turn(address: SocketAddr, session_id: &str) {
    let body = json!({
        "model": SLUG,
        "max_tokens": 16,
        "stream": true,
        "messages": [{"role": "user", "content": "hi"}],
    })
    .to_string();
    let mut stream = TcpStream::connect(address).expect("connect gateway");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("gateway read timeout");
    write!(
        stream,
        "POST /v1/messages HTTP/1.1\r\nhost: {address}\r\ncontent-type: application/json\r\n\
         anthropic-version: 2023-06-01\r\nx-session-id: {session_id}\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("write gateway request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read gateway response");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "gateway response: {response}"
    );
}

fn wait_for_usage_turn(root: &Path) {
    let path = root.join("usage.sqlite3");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(connection) =
            rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            && let Ok(count) =
                connection.query_row("SELECT COUNT(*) FROM token_usage_turns", [], |row| {
                    row.get::<_, i64>(0)
                })
            && count > 0
        {
            return;
        }
        if Instant::now() >= deadline {
            eprintln!(
                "root files: {:?}",
                std::fs::read_dir(root).map(|entries| entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
                    .collect::<Vec<_>>())
            );
            for name in ["gateway.stdout", "gateway.stderr"] {
                if let Ok(log) = std::fs::read_to_string(root.join(name)) {
                    eprintln!("--- {name} ---\n{log}");
                }
            }
            panic!("gateway did not record a usage turn in {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct GatewayProcess(std::process::Child);

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_hook(root: &Path, event: &str, body: &Value) {
    run_hook_with(root, event, body, |_| {});
}

fn run_hook_with(
    root: &Path,
    event: &str,
    body: &Value,
    configure: impl FnOnce(&mut std::process::Command),
) {
    let mut command = common::cli(root);
    command
        .args(["report-hook", "--event", event])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure(&mut command);
    let mut child = command.spawn().expect("spawn report hook");
    child
        .stdin
        .take()
        .expect("hook stdin")
        .write_all(body.to_string().as_bytes())
        .expect("write hook body");
    let output = child.wait_with_output().expect("wait for report hook");
    assert!(
        output.status.success(),
        "{event} hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn report_log(root: &Path) -> String {
    std::fs::read_to_string(root.join("report-hook.log")).unwrap_or_default()
}

fn persisted_model(root: &Path, session_id: &str) -> Option<String> {
    report_log(root)
        .lines()
        .find(|line| {
            line.contains("DUCX report event persisted")
                && line.contains(&format!("session_id=\"{session_id}\""))
        })
        .and_then(|line| line.split("model=\"").nth(1))
        .map(|rest| rest.split('"').next().unwrap_or_default().to_owned())
}

#[test]
fn client_model_names_resolve_to_the_catalog_slug() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-one",
            "model": "DeepSeek-V4.1-Flash[1m]",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-one").as_deref(), Some(SLUG));

    let route = format!(
        "{}[1m]",
        codex_mixin::clients::claude_desktop::route_id(SLUG)
    );
    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-two",
            "model": route,
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-two").as_deref(), Some(SLUG));
}

#[test]
fn session_start_seeds_the_model_for_later_events() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    run_hook(
        root,
        "session-start",
        &json!({
            "session_id": "session-three",
            "model": "DeepSeek-V4.1-Flash[1m]",
            "cwd": "/tmp",
        }),
    );
    assert!(root.join("ducx-report-models/session-three.json").is_file());

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-three",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(
        persisted_model(root, "session-three").as_deref(),
        Some(SLUG)
    );

    let skill = root.join("codex/skills/archify");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "# Archify\n").unwrap();
    let tool = json!({
        "session_id": "session-three",
        "tool_name": "Read",
        "tool_input": {"file_path": skill.join("SKILL.md")},
        "cwd": root,
    });
    run_hook(root, "post-tool-use", &tool);
    run_hook(root, "post-tool-use", &tool);
    run_hook(
        root,
        "user-prompt-submit",
        &json!({"session_id": "session-three", "prompt": "next turn", "cwd": root}),
    );
    run_hook(root, "post-tool-use", &tool);
    let log = report_log(root);
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("DUCX report event persisted")
                && line.contains("event=\"post-tool-use\""))
            .count(),
        2,
        "a skill must be queued once in each prompt turn: {log}"
    );
    let skill_reports = std::fs::read_dir(root.join("ducx-report-queue"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<Value>(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .filter(|report| report["event"] == "post-tool-use")
        .collect::<Vec<_>>();
    assert_eq!(skill_reports.len(), 2);
    for report in &skill_reports {
        assert_eq!(report["hook_body"]["skill_name"], "archify");
        assert_eq!(report["hook_body"]["execution_mode"], "auto");
        assert_eq!(report["hook_body"]["trigger_kind"], "skillmd");
        assert!(report["hook_body"]["turn_id"].as_str().is_some());
    }
    assert_ne!(
        skill_reports[0]["hook_body"]["turn_id"],
        skill_reports[1]["hook_body"]["turn_id"]
    );
}

#[test]
fn usage_ledger_resolves_events_without_a_model() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let upstream = spawn_mock_anthropic();
    write_gateway_reporting_config(root, upstream);

    let bind = format!("127.0.0.1:{}", free_port());
    let address: SocketAddr = bind.parse().expect("gateway bind address");
    let stdout = std::fs::File::create(root.join("gateway.stdout")).unwrap();
    let stderr = std::fs::File::create(root.join("gateway.stderr")).unwrap();
    let mut command = common::cli(root);
    command
        .args(["serve", "--bind", &bind])
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let gateway = GatewayProcess(command.spawn().expect("spawn gateway"));
    wait_for_gateway(address);
    send_gateway_turn(address, "session-four");
    wait_for_usage_turn(root);
    drop(gateway);

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-four",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-four").as_deref(), Some(SLUG));
}

#[test]
fn dsh_session_log_resolves_events_without_a_model() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    let session_dir = root
        .join("home/.dsh/sessions/--tmp-workspace--")
        .join("session-session-five");
    std::fs::create_dir_all(&session_dir).unwrap();
    let record = serde_json::to_vec(&json!({
        "type": "turn",
        "data": {"model": SLUG},
    }))
    .unwrap();
    let mut compressed = Vec::new();
    {
        let mut encoder = zstd::stream::write::Encoder::new(&mut compressed, 3).unwrap();
        encoder.write_all(&record).unwrap();
        encoder.write_all(b"\n").unwrap();
        encoder.finish().unwrap();
    }
    std::fs::write(session_dir.join("session.v4.jsonl.zstd"), compressed).unwrap();

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-five",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-five").as_deref(), Some(SLUG));
}

#[test]
fn model_switch_updates_the_cache_without_reporting() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    run_hook(
        root,
        "model-switch",
        &json!({
            "session_id": "session-six",
            "from_model": "Claude Haiku 5.5-baidu-oneapi[1m]",
            "to_model": SLUG,
            "cwd": "/tmp",
        }),
    );
    assert!(root.join("ducx-report-models/session-six.json").is_file());
    assert!(persisted_model(root, "session-six").is_none());

    run_hook(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-six",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
    );
    assert_eq!(persisted_model(root, "session-six").as_deref(), Some(SLUG));
}

#[test]
fn dsh_connect_mounts_and_removes_the_reporting_bridge() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);

    let dsh_home = root.join("home/.dsh");
    let patch_path = dsh_home.join("profiles/desktop/cordis.patch.yml");
    std::fs::create_dir_all(patch_path.parent().unwrap()).unwrap();
    std::fs::write(&patch_path, "- id: user-entry\n  config: {}\n").unwrap();
    // The bridge and its directly imported peers are pre-installed so the test
    // never shells out to a real DSH CLI/pnpm on machines that have DSH.
    for package in ["dsh-hooks-claude-code", "dsh-hook-protocol", "dsh-llm"] {
        let package_dir = dsh_home
            .join("profiles/node_modules/@deepseek-ai")
            .join(package);
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(package_dir.join("package.json"), "{}").unwrap();
    }

    let output = common::cli(root)
        .args(["--no-tui", "--json-errors", "connect", "dsh"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "connect dsh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let patch = std::fs::read_to_string(&patch_path).unwrap();
    let parsed: serde_yaml::Value = serde_yaml::from_str(&patch).unwrap();
    let row = managed_row(&parsed).expect("reporting bridge row");
    assert_eq!(
        row.get("name").and_then(serde_yaml::Value::as_str),
        Some("@deepseek-ai/dsh-hooks-claude-code")
    );
    let config_path = row
        .get("config")
        .and_then(|config| config.get("configPath"))
        .and_then(serde_yaml::Value::as_str)
        .expect("reporting bridge configPath");
    assert_eq!(Path::new(config_path), dsh_home.join("hooks.json"));
    assert!(patch.contains("user-entry"), "patch: {patch}");

    // Re-running connect must not duplicate the managed row.
    let output = common::cli(root)
        .args(["--no-tui", "--json-errors", "connect", "dsh"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "second connect dsh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let patch = std::fs::read_to_string(&patch_path).unwrap();
    assert_eq!(patch.matches("codex-mixin-report-hooks").count(), 1);

    let output = common::cli(root)
        .args(["--no-tui", "--json-errors", "connect", "remove", "dsh"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "connect remove dsh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let patch = std::fs::read_to_string(&patch_path).unwrap();
    assert!(
        !patch.contains("codex-mixin-report-hooks"),
        "patch: {patch}"
    );
    assert!(patch.contains("user-entry"), "patch: {patch}");
}

#[cfg(unix)]
#[test]
fn read_only_state_dir_falls_back_to_the_temp_area() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_reporting_config(root);
    let temp_area = root.join("tmp");
    std::fs::create_dir_all(&temp_area).unwrap();

    let original = std::fs::metadata(root).unwrap().permissions();
    let mut read_only = original.clone();
    read_only.set_mode(0o500);
    std::fs::set_permissions(root, read_only).unwrap();

    run_hook_with(
        root,
        "user-prompt-submit",
        &json!({
            "session_id": "session-seven",
            "model": "DeepSeek-V4.1-Flash[1m]",
            "prompt": "hi",
            "cwd": "/tmp",
        }),
        |command| {
            command.env("TMPDIR", &temp_area);
        },
    );

    std::fs::set_permissions(root, original).unwrap();

    let log = std::fs::read_to_string(temp_area.join("codex-mixin-report/report-hook.log"))
        .expect("fallback report log");
    assert!(log.contains("DUCX report event persisted"), "log: {log}");
}

fn managed_row(patch: &serde_yaml::Value) -> Option<&serde_yaml::Value> {
    patch.as_sequence()?.iter().find_map(|entry| {
        entry.get("insert")?.as_sequence()?.iter().find(|row| {
            row.get("id").and_then(serde_yaml::Value::as_str) == Some("codex-mixin-report-hooks")
        })
    })
}
