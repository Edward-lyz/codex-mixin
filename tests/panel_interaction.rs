//! Exercise the browser action transport through real isolated CLI children.
use std::process::Stdio;
use std::time::Duration;

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
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
        Self {
            child,
            url,
            token,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.url))
            .bearer_auth(&self.token)
    }
}

#[tokio::test]
async fn saved_provider_progress_arrives_before_discovery_and_cancel_releases_lock() {
    let root = tempfile::tempdir().unwrap();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let panel = Panel::start(root.path()).await;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        panel
            .post("/api/actions/stream")
            .json(&json!({
                "action": "provider_add",
                "params": {"preset": "baidu-oneapi", "options": {
                    "key": "fixture-secret", "quota_username": "fixture-user",
                    "base_url": format!("http://{}", upstream.local_addr().unwrap())
                }}
            }))
            .send(),
    )
    .await
    .expect("action must respond before discovery finishes")
    .unwrap();
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
        .post("/api/actions/stream")
        .json(&json!({"action":"provider_disable", "params":{"id":"baidu-oneapi"}}))
        .send()
        .await
        .unwrap();
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
    let response = panel.post("/api/actions/stream").json(&json!({"action":"provider_update","params":{"id":"baidu-oneapi","options":{"baidu_auth_bridge":"ducx_loopback"}}})).send().await.unwrap();
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
    let terminal: Value = serde_json::from_str(output.lines().last().unwrap()).unwrap();
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
        panel.post("/api/actions/stream").json(&json!({"action":"provider_add","params":{"preset":"baidu-oneapi","options":{"key":"fixture-secret","quota_username":"fixture-user","base_url":format!("http://{address}")}}})).send().await.unwrap().text().await.unwrap()
    }).await.unwrap();
    server.await.unwrap();
    let terminal: Value = serde_json::from_str(output.lines().last().unwrap()).unwrap();
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
    let response = panel.post("/api/actions/stream").json(&json!({"action":"provider_update","params":{"id":"baidu-oneapi","options":{"baidu_auth_bridge":"ducx_loopback"}}})).send().await.unwrap();
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
