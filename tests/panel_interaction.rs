//! Exercise the browser action transport through real isolated CLI children.
use std::process::Stdio;
use std::time::Duration;

#[cfg(unix)]
use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
#[cfg(unix)]
use codex_mixin::provider::baidu_oneapi_provider;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

mod common;

struct Panel {
    child: tokio::process::Child,
    url: String,
    token: String,
    client: reqwest::Client,
    log: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Drop for Panel {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            let _ = codex_mixin::platform::kill_process_tree(pid);
        }
    }
}

impl Panel {
    async fn start(root: &std::path::Path) -> Self {
        let mut command = common::tokio_cli(root);
        codex_mixin::platform::isolate_tokio_process_group(&mut command);
        let mut child = command
            .args(["panel", "--bind", "127.0.0.1:0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let (url, token) = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(line) = lines.next_line().await.unwrap() {
                if let Some((url, fragment)) = line
                    .trim()
                    .strip_prefix("open ")
                    .and_then(|line| line.split_once("/#"))
                {
                    return (
                        url.to_owned(),
                        fragment.split_once("token=").unwrap().1.to_owned(),
                    );
                }
            }
            panic!("panel exited before advertising its address");
        })
        .await
        .unwrap();
        // Keep draining stderr: closing the pipe makes later `eprintln!` calls
        // in the panel fail, and the retained lines explain request failures.
        let log = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = std::sync::Arc::clone(&log);
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                let mut sink = sink.lock().unwrap();
                sink.push_str(&line);
                sink.push('\n');
            }
        });
        Self {
            child,
            url,
            token,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            log,
        }
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.url))
            .bearer_auth(&self.token)
    }

    async fn stream_action(&self, body: Value) -> reqwest::Response {
        self.post("/api/actions/stream")
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "panel rejected the action stream: {error}\npanel log:\n{}",
                    self.log.lock().unwrap()
                )
            })
    }
}

/// `streamAction` in `panel/assets/app.js` reads these fields directly, so
/// pin the wire shape here: a rename on either side breaks the panel without
/// any Rust type error.
fn assert_stream_contract(output: &str) -> Vec<Value> {
    let events = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("invalid NDJSON event {line:?}: {error}"))
        })
        .collect::<Vec<_>>();
    for event in &events {
        let kind = event["type"]
            .as_str()
            .unwrap_or_else(|| panic!("stream event without a type: {event}"));
        match kind {
            "step" => assert!(
                event["index"].is_number() && event["total"].is_number(),
                "{event}"
            ),
            "output" => {
                assert!(event["text"].is_string(), "{event}");
                assert!(
                    matches!(event["channel"].as_str(), Some("stdout" | "stderr")),
                    "{event}"
                );
            }
            "heartbeat" => {}
            "result" => assert!(
                event.get("stdout").is_some() && event.get("json").is_some(),
                "{event}"
            ),
            "error" => assert!(event["error"].is_object(), "{event}"),
            other => panic!("unexpected stream event type: {other}"),
        }
    }
    let client = include_str!("../panel/assets/app.js");
    for kind in ["output", "result", "error"] {
        assert!(
            client.contains(&format!("event.type === \"{kind}\"")),
            "panel/assets/app.js no longer handles {kind} events"
        );
    }
    events
}

#[tokio::test]
async fn saved_provider_progress_arrives_before_discovery_and_cancel_releases_lock() {
    let root = tempfile::tempdir().unwrap();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let panel = Panel::start(root.path()).await;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        panel.stream_action(json!({
            "action": "provider_add",
            "params": {"preset": "baidu-oneapi", "options": {
                "key": "fixture-secret", "quota_username": "fixture-user",
                "base_url": format!("http://{}", upstream.local_addr().unwrap())
            }}
        })),
    )
    .await
    .expect("action must respond before discovery finishes");
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("application/x-ndjson")
    );
    let mut stream = response.bytes_stream();
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !output.contains("provider added: baidu-oneapi") {
            output.push_str(&String::from_utf8_lossy(
                &stream.next().await.unwrap().unwrap(),
            ));
        }
    })
    .await
    .expect("saved state must reach the browser before a slow model lookup");
    let (_connection, _) = tokio::time::timeout(Duration::from_secs(3), upstream.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(root.path().join("config.json").exists());
    assert!(!output.contains("fixture-secret"));
    let busy = panel
        .stream_action(json!({"action":"provider_disable", "params":{"id":"baidu-oneapi"}}))
        .await;
    assert_eq!(busy.status(), 409);
    assert_eq!(
        panel
            .post("/api/actions/cancel")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(chunk) = stream.next().await {
            output.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        }
    })
    .await
    .expect("cancel must finish the stream");
    let terminal: Value = serde_json::from_str(output.lines().last().unwrap()).unwrap();
    assert_eq!(terminal["type"], "error");
    assert_eq!(terminal["error"]["code"], "cancelled");
    assert!(terminal["error"]["committed"].is_null());
    let result = panel
        .post("/api/actions")
        .json(&json!({"action":"provider_disable", "params":{"id":"baidu-oneapi"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        result.status(),
        200,
        "cancelled action must release its lock"
    );
}

#[tokio::test]
async fn streaming_endpoint_requires_authentication() {
    let root = tempfile::tempdir().unwrap();
    let panel = Panel::start(root.path()).await;
    let response = panel
        .client
        .post(format!("{}/api/actions/stream", panel.url))
        .json(&json!({"action":"provider_disable","params":{"id":"test"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}

#[cfg(unix)]
#[tokio::test]
async fn headless_ducx_qr_is_streamed_and_login_completes() {
    let root = tempfile::tempdir().unwrap();
    let executable = root
        .path()
        .join("home/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    // The real DUCX supports pipe output. This controlled launcher exercises
    // the same CLI login path without a network request or real identity.
    std::fs::write(&executable, concat!(
        "#!/bin/sh\n",
        "printf 'Scan this fixture QR\\n\u{2588}\u{2588}  \u{2588}\u{2588}\\n  \u{2588}\u{2588}  \\n'\n",
        "sleep 1\n",
        "mkdir -p \"$HOME/.comate/login-user\"\n",
        "printf 'eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJmaXh0dXJlIiwiaWF0IjoxNzg2NjAwMDAwfQ.signature' > \"$HOME/.comate/login-user/fixture\"\n",
    )).unwrap();
    codex_mixin::platform::make_executable(&executable).unwrap();
    let mut provider = baidu_oneapi_provider("baidu-oneapi", "fixture-key");
    provider.quota_username = Some("fixture-user".to_owned());
    save_stored_config_to_path(
        &root.path().join("config.json"),
        &StoredGatewayConfig {
            providers: vec![provider],
            ..Default::default()
        },
    )
    .unwrap();
    let panel = Panel::start(root.path()).await;
    let response = panel
        .stream_action(json!({"action":"provider_update","params":{"id":"baidu-oneapi","options":{"baidu_auth_bridge":"ducx_loopback"}}}))
        .await;
    assert_eq!(response.status(), 200);
    let mut stream = response.bytes_stream();
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = stream.next().await {
            output.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        }
    })
    .await
    .unwrap();
    assert!(
        output.contains("\u{2588}\u{2588}  \u{2588}\u{2588}"),
        "QR output must reach the browser: {output}"
    );
    assert!(!output.contains("interactive terminal"), "{output}");
    let terminal = assert_stream_contract(&output).pop().unwrap();
    assert_eq!(terminal["type"], "result", "{output}");
    assert_eq!(terminal["ok"], true);

    std::fs::remove_file(
        root.path()
            .join("home/.codex-mixin/ducx/home/.comate/login-user/fixture"),
    )
    .unwrap();
    let cli = common::tokio_cli(root.path())
        .args(["--no-tui", "--json-errors", "connect", "ducx", "--json"])
        .env("CODEX_MIXIN_LOGIN_STDIO", "1")
        .output()
        .await
        .unwrap();
    assert!(cli.status.success());
    let result: Value =
        serde_json::from_slice(&cli.stdout).expect("login must preserve JSON stdout");
    assert_eq!(result["executable"], executable.to_str().unwrap());
    assert!(String::from_utf8_lossy(&cli.stderr).contains("Scan this fixture QR"));
}

#[tokio::test]
async fn discovery_failure_preserves_committed_stage() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let root = tempfile::tempdir().unwrap();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = upstream.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = upstream.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
    });
    let panel = Panel::start(root.path()).await;
    let output = tokio::time::timeout(Duration::from_secs(5), async {
        panel.stream_action(json!({"action":"provider_add","params":{"preset":"baidu-oneapi","options":{"key":"fixture-secret","quota_username":"fixture-user","base_url":format!("http://{address}")}}})).await.text().await.unwrap()
    }).await.unwrap();
    server.await.unwrap();
    let terminal = assert_stream_contract(&output).pop().unwrap();
    assert_eq!(terminal["type"], "error");
    assert_eq!(terminal["error"]["committed"], true);
    assert_eq!(terminal["error"]["stage"], "model discovery");
    assert!(root.path().join("config.json").exists());
    assert!(!output.contains("fixture-secret"));
}

#[cfg(unix)]
#[tokio::test]
async fn browser_disconnect_terminates_nested_login_and_releases_lock() {
    let root = tempfile::tempdir().unwrap();
    let executable = root
        .path()
        .join("home/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, "#!/bin/sh\necho $$ > \"$HOME/login.pid\"\nprintf 'waiting-for-fixture-login\\n'\nexec sleep 60\n").unwrap();
    codex_mixin::platform::make_executable(&executable).unwrap();
    let mut provider = baidu_oneapi_provider("baidu-oneapi", "fixture-key");
    provider.quota_username = Some("fixture-user".to_owned());
    save_stored_config_to_path(
        &root.path().join("config.json"),
        &StoredGatewayConfig {
            providers: vec![provider],
            ..Default::default()
        },
    )
    .unwrap();
    let panel = Panel::start(root.path()).await;
    let response = panel
        .stream_action(json!({"action":"provider_update","params":{"id":"baidu-oneapi","options":{"baidu_auth_bridge":"ducx_loopback"}}}))
        .await;
    assert_eq!(response.status(), 200);
    let mut stream = response.bytes_stream();
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !output.contains("waiting-for-fixture-login") {
            output.push_str(&String::from_utf8_lossy(
                &stream.next().await.unwrap().unwrap(),
            ));
        }
    })
    .await
    .unwrap();
    let pid =
        std::fs::read_to_string(root.path().join("home/.codex-mixin/ducx/home/login.pid")).unwrap();
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = panel
                .post("/api/actions")
                .json(&json!({"action":"provider_disable","params":{"id":"baidu-oneapi"}}))
                .send()
                .await
                .unwrap();
            if response.status() != 409 {
                assert_eq!(response.status(), 200);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        loop {
            let alive = tokio::process::Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .await
                .unwrap()
                .success();
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("disconnect must reap the nested login and unlock actions");
}
