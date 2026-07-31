use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, OriginalUri, State};
use axum::http::{HeaderMap, Method, Response, StatusCode, header};
use axum::routing::any;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use uuid::Uuid;

const REQUEST_BODY_LIMIT: usize = 16 * 1024 * 1024;
const DUCC_HEADER_NAME: &str = "comate_custom_header";
const LOOPBACK_API_KEY: &str = "codex-mixin-loopback";

pub(crate) fn default_ducc_executable() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".codex-mixin/ducc/home/.baidu-cc/baidu-cc/bin/ducc"))
        .filter(|path| path.is_file())
}

fn managed_home(executable: &Path) -> anyhow::Result<PathBuf> {
    let bin = executable
        .parent()
        .context("DUCC executable has no bin directory")?;
    let install = bin
        .parent()
        .context("DUCC executable has no install directory")?;
    let dot_baidu = install
        .parent()
        .context("DUCC executable has no .baidu-cc directory")?;
    let home = dot_baidu
        .parent()
        .context("DUCC executable has no managed HOME")?;
    ensure!(
        bin.file_name().and_then(|value| value.to_str()) == Some("bin")
            && install.file_name().and_then(|value| value.to_str()) == Some("baidu-cc")
            && dot_baidu.file_name().and_then(|value| value.to_str()) == Some(".baidu-cc"),
        "DUCC executable must use the managed HOME/.baidu-cc/baidu-cc/bin layout"
    );
    Ok(home.to_owned())
}

struct RelayPolicy {
    marker: String,
    skip_matching_posts: usize,
    target: reqwest::Url,
    body: Value,
    headers: HeaderMap,
    response: oneshot::Sender<reqwest::Response>,
}

#[derive(Clone)]
struct BridgeState {
    route_token: Arc<str>,
    client: reqwest::Client,
    policies: Arc<Mutex<HashMap<String, RelayPolicy>>>,
}

struct DuccBridge {
    base_url: String,
    state: BridgeState,
    shutdown: watch::Sender<bool>,
}

impl DuccBridge {
    async fn spawn(client: reqwest::Client) -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind DUCC loopback bridge")?;
        let address = listener
            .local_addr()
            .context("read DUCC loopback bridge address")?;
        let route_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let state = BridgeState {
            route_token: Arc::from(route_token.as_str()),
            client,
            policies: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = Router::new()
            .route(
                "/{*path}",
                any(forward).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
            )
            .with_state(state.clone());
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await;
            if let Err(error) = result {
                tracing::warn!(%error, "DUCC loopback bridge stopped unexpectedly");
            }
        });
        Ok(Self {
            base_url: format!("http://{address}/{route_token}"),
            state,
            shutdown,
        })
    }

    async fn register(
        &self,
        model: &str,
        target: reqwest::Url,
        body: Value,
        headers: HeaderMap,
        skip_matching_posts: usize,
    ) -> anyhow::Result<(String, oneshot::Receiver<reqwest::Response>)> {
        ensure!(body.is_object(), "DUCC relay body must be an object");
        let marker = format!(
            "codex-mixin-loopback-{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let (sender, receiver) = oneshot::channel();
        let mut policies = self.state.policies.lock().await;
        ensure!(
            !policies.contains_key(model),
            "DUCC model {model} already has an active loopback request"
        );
        policies.insert(
            model.to_owned(),
            RelayPolicy {
                marker: marker.clone(),
                skip_matching_posts,
                target,
                body,
                headers,
                response: sender,
            },
        );
        Ok((marker, receiver))
    }

    async fn cancel(&self, model: &str, marker: &str) {
        let mut policies = self.state.policies.lock().await;
        if policies
            .get(model)
            .is_some_and(|policy| policy.marker == marker)
        {
            policies.remove(model);
        }
    }
}

impl Drop for DuccBridge {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn forward(
    State(state): State<BridgeState>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if !uri
        .path()
        .strip_prefix('/')
        .is_some_and(|path| path.starts_with(state.route_token.as_ref()))
    {
        return static_response(StatusCode::NOT_FOUND, Body::empty(), None);
    }
    if method == Method::HEAD {
        return static_response(StatusCode::OK, Body::empty(), None);
    }
    if method != Method::POST {
        return static_response(StatusCode::METHOD_NOT_ALLOWED, Body::empty(), None);
    }
    match forward_post(state, headers, body).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "DUCC loopback bridge rejected request");
            static_response(
                StatusCode::BAD_GATEWAY,
                Body::from(
                    r#"{"type":"error","error":{"message":"DUCC loopback rejected request"}}"#,
                ),
                Some("application/json"),
            )
        }
    }
}

async fn forward_post(
    state: BridgeState,
    mut native_headers: HeaderMap,
    body: Bytes,
) -> anyhow::Result<Response<Body>> {
    let payload: Value = serde_json::from_slice(&body).context("decode DUCC Messages request")?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let marker = {
        let mut policies = state.policies.lock().await;
        let Some(policy) = policies.get_mut(model) else {
            return Ok(synthetic_messages_response());
        };
        // A fresh DUCC process opens one internal DeepSeek helper request
        // before its first user turn. If the requested model is that same
        // model, skip exactly one matching POST before considering the marker.
        if policy.skip_matching_posts > 0 {
            policy.skip_matching_posts -= 1;
            return Ok(synthetic_messages_response());
        }
        policy.marker.clone()
    };
    if !contains_string(&payload, &marker) {
        return Ok(synthetic_messages_response());
    }

    // The policy is consumed before network I/O. A retry or replay therefore
    // cannot create a second upstream inference request.
    let policy = state
        .policies
        .lock()
        .await
        .remove(model)
        .context("DUCC loopback policy was already consumed")?;
    ensure!(
        native_headers.contains_key(DUCC_HEADER_NAME),
        "DUCC request is missing its native authentication header"
    );
    // Claude Code requires an API key before it will send to a non-official
    // loopback base URL. The placeholder exists only to unlock that local
    // request and must never reach the real upstream.
    native_headers.remove(header::AUTHORIZATION);
    native_headers.remove("x-api-key");
    strip_hop_by_hop_headers(&mut native_headers);
    for (name, value) in policy.headers {
        if let Some(name) = name {
            ensure!(
                name != header::AUTHORIZATION
                    && name.as_str() != "x-api-key"
                    && name.as_str() != DUCC_HEADER_NAME,
                "gateway headers cannot replace DUCC authentication"
            );
            native_headers.insert(name, value);
        }
    }
    strip_hop_by_hop_headers(&mut native_headers);
    let encoded = serde_json::to_vec(&policy.body).context("encode sanitized DUCC relay body")?;
    let upstream = state
        .client
        .post(policy.target)
        .headers(native_headers)
        .body(encoded)
        .send()
        .await
        .context("forward DUCC-authenticated request")?;
    policy
        .response
        .send(upstream)
        .map_err(|_| anyhow::anyhow!("DUCC gateway response receiver closed"))?;
    Ok(synthetic_messages_response())
}

fn contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value.contains(needle),
        Value::Array(values) => values.iter().any(|value| contains_string(value, needle)),
        Value::Object(values) => values.values().any(|value| contains_string(value, needle)),
        _ => false,
    }
}

fn strip_hop_by_hop_headers(headers: &mut HeaderMap) {
    for name in [
        header::HOST,
        header::CONTENT_LENGTH,
        header::CONNECTION,
        header::TRANSFER_ENCODING,
        header::TE,
        header::TRAILER,
        header::UPGRADE,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
    ] {
        headers.remove(name);
    }
}

fn synthetic_messages_response() -> Response<Body> {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_codex_mixin_loopback\",",
        "\"type\":\"message\",\"role\":\"assistant\",\"model\":\"ducc-loopback\",",
        "\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,",
        "\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,",
        "\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,",
        "\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",",
        "\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    static_response(StatusCode::OK, Body::from(body), Some("text/event-stream"))
}

fn static_response(status: StatusCode, body: Body, content_type: Option<&str>) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder
        .body(body)
        .expect("static DUCC loopback response is valid")
}

struct DuccClient {
    stdin: Mutex<ChildStdin>,
    output: Mutex<mpsc::Receiver<Value>>,
    session_id: String,
    running: Arc<AtomicBool>,
    shutdown: watch::Sender<bool>,
}

impl DuccClient {
    async fn spawn(
        executable: &Path,
        home: &Path,
        cwd: &Path,
        model: &str,
        base_url: &str,
    ) -> anyhow::Result<Self> {
        let settings = serde_json::to_string(&json!({
            "env": {
                "ANTHROPIC_BASE_URL": base_url,
                "ANTHROPIC_API_KEY": LOOPBACK_API_KEY
            }
        }))
        .expect("static DUCC settings are serializable");
        let mut child = Command::new(executable)
            .args([
                "--bare",
                "--no-ducc-system-prompt",
                "--disable-slash-commands",
                "--no-session-persistence",
                "--permission-mode",
                "dontAsk",
                "--prompt-suggestions",
                "false",
                "--tools",
                "",
                "--model",
                model,
                "--settings",
                &settings,
                "--print",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
            ])
            .env("HOME", home)
            .env("BAIDU_CC_PLATFORM", "AIIDE-terminal")
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .env("DISABLE_DUCC_CLI_UPDATE", "1")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start managed DUCC {}", executable.display()))?;
        let stdin = child.stdin.take().context("capture DUCC stdin")?;
        let stdout = child.stdout.take().context("capture DUCC stdout")?;
        let stderr = child.stderr.take().context("capture DUCC stderr")?;
        let (sender, receiver) = mpsc::channel(256);
        let running = Arc::new(AtomicBool::new(true));
        let reader_running = Arc::clone(&running);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(message) = serde_json::from_str::<Value>(&line)
                    && sender.send(message).await.is_err()
                {
                    break;
                }
            }
            reader_running.store(false, Ordering::Release);
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(_)) = lines.next_line().await {
                tracing::debug!("managed DUCC emitted a stderr line");
            }
        });
        let waiter_running = Arc::clone(&running);
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {}
                changed = shutdown_rx.changed() => {
                    if changed.is_ok() && *shutdown_rx.borrow() {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                }
            }
            waiter_running.store(false, Ordering::Release);
        });
        Ok(Self {
            stdin: Mutex::new(stdin),
            output: Mutex::new(receiver),
            session_id: Uuid::new_v4().to_string(),
            running,
            shutdown,
        })
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn shutdown(&self) {
        self.running.store(false, Ordering::Release);
        let _ = self.shutdown.send(true);
    }

    async fn trigger(&self, marker: &str) -> anyhow::Result<()> {
        let message = json!({
            "type": "user",
            "session_id": self.session_id,
            "parent_tool_use_id": null,
            "message": {
                "role": "user",
                "content": [{"type":"text","text":marker}]
            }
        });
        let mut encoded = serde_json::to_vec(&message).context("encode DUCC stream input")?;
        encoded.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&encoded)
            .await
            .context("write DUCC stream input")?;
        stdin.flush().await.context("flush DUCC stream input")
    }

    async fn wait_for_result(&self) -> anyhow::Result<()> {
        let mut output = self.output.lock().await;
        while let Some(message) = output.recv().await {
            if message.get("type").and_then(Value::as_str) == Some("result") {
                if message.get("is_error").and_then(Value::as_bool) == Some(true) {
                    let detail = message
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown DUCC error");
                    anyhow::bail!("managed DUCC turn failed: {detail}");
                }
                return Ok(());
            }
        }
        anyhow::bail!("managed DUCC output closed before turn result")
    }
}

impl Drop for DuccClient {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

pub(crate) struct DuccRuntime {
    executable: PathBuf,
    home: PathBuf,
    cwd: tempfile::TempDir,
    bridge: DuccBridge,
    client: Mutex<Option<(String, Arc<DuccClient>)>>,
    dispatch: Arc<Mutex<()>>,
}

impl DuccRuntime {
    pub(crate) async fn spawn(
        executable: PathBuf,
        client: reqwest::Client,
    ) -> anyhow::Result<Self> {
        ensure!(
            executable.is_file(),
            "DUCC executable does not exist: {}",
            executable.display()
        );
        let home = managed_home(&executable)?;
        let cwd = tempfile::tempdir().context("create isolated DUCC working directory")?;
        let bridge = DuccBridge::spawn(client).await?;
        Ok(Self {
            executable,
            home,
            cwd,
            bridge,
            client: Mutex::new(None),
            dispatch: Arc::new(Mutex::new(())),
        })
    }

    async fn client_for_model(&self, model: &str) -> anyhow::Result<(Arc<DuccClient>, bool)> {
        let mut slot = self.client.lock().await;
        if let Some((current_model, client)) = slot.as_ref()
            && current_model == model
            && client.is_running()
        {
            return Ok((Arc::clone(client), false));
        }
        let client = Arc::new(
            DuccClient::spawn(
                &self.executable,
                &self.home,
                self.cwd.path(),
                model,
                &self.bridge.base_url,
            )
            .await?,
        );
        *slot = Some((model.to_owned(), Arc::clone(&client)));
        Ok((client, true))
    }

    pub(crate) async fn send(
        &self,
        model: &str,
        target: reqwest::Url,
        body: Value,
        headers: HeaderMap,
        timeout: Duration,
    ) -> anyhow::Result<reqwest::Response> {
        // DUCC is roughly 360 MiB per process. One serialized, long-lived
        // worker avoids multiplying gateway memory by model or concurrency.
        let dispatch_guard = Arc::clone(&self.dispatch).lock_owned().await;
        let (client, spawned) = self.client_for_model(model).await?;
        ensure!(client.is_running(), "managed DUCC process is not running");
        let (marker, response) = self
            .bridge
            .register(
                model,
                target,
                body,
                headers,
                usize::from(spawned && model.eq_ignore_ascii_case("DeepSeek-V4-Flash")),
            )
            .await?;
        if let Err(error) = client.trigger(&marker).await {
            self.bridge.cancel(model, &marker).await;
            client.shutdown();
            return Err(error);
        }
        let response = {
            let early_result = client.wait_for_result();
            tokio::pin!(early_result);
            tokio::select! {
                response = response => match response {
                    Ok(response) => response,
                    Err(_) => {
                        self.bridge.cancel(model, &marker).await;
                        client.shutdown();
                        anyhow::bail!("DUCC loopback response channel closed")
                    }
                },
                result = &mut early_result => {
                    self.bridge.cancel(model, &marker).await;
                    client.shutdown();
                    match result {
                        Ok(()) => anyhow::bail!(
                            "managed DUCC turn completed without opening its authenticated request"
                        ),
                        Err(error) => return Err(error),
                    }
                },
                _ = tokio::time::sleep(timeout) => {
                    self.bridge.cancel(model, &marker).await;
                    client.shutdown();
                    anyhow::bail!("DUCC did not open its authenticated request in time")
                }
            }
        };
        tokio::spawn(async move {
            let _dispatch_guard = dispatch_guard;
            match tokio::time::timeout(timeout, client.wait_for_result()).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    client.shutdown();
                    tracing::warn!(error = %format!("{error:#}"), "managed DUCC turn did not complete");
                }
                Err(_) => {
                    client.shutdown();
                    tracing::warn!("managed DUCC turn result timed out");
                }
            }
        });
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::routing::post;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn derives_managed_home_without_reading_auth_files() {
        let executable = Path::new("/tmp/codex-mixin/ducc/home/.baidu-cc/baidu-cc/bin/ducc");
        assert_eq!(
            managed_home(executable).unwrap(),
            Path::new("/tmp/codex-mixin/ducc/home")
        );
    }

    #[test]
    fn marker_search_handles_nested_message_content() {
        let payload = json!({
            "messages": [{"role":"user","content":[{"type":"text","text":"prefix marker suffix"}]}]
        });
        assert!(contains_string(&payload, "marker"));
        assert!(!contains_string(&payload, "missing"));
    }

    #[tokio::test]
    async fn reports_ducc_failure_before_loopback_timeout() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("home/.baidu-cc/baidu-cc/bin/ducc");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' '{"type":"result","is_error":true,"result":"Not logged in"}'
  exit 0
done
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let runtime = DuccRuntime::spawn(executable, reqwest::Client::new())
            .await
            .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            runtime.send(
                "GLM-5.2",
                "http://127.0.0.1:9/v1/messages".parse().unwrap(),
                json!({"model":"GLM-5.2","messages":[]}),
                HeaderMap::new(),
                Duration::from_secs(30),
            ),
        )
        .await
        .expect("DUCC failure must beat the loopback timeout")
        .unwrap_err();
        assert!(
            format!("{result:#}").contains("Not logged in"),
            "unexpected DUCC error: {result:#}"
        );
    }

    #[tokio::test]
    async fn bridge_skips_auxiliary_and_replaces_the_entire_multimodal_body_once() {
        let captured = Arc::new(Mutex::new(Vec::<(Value, bool, bool, Option<String>)>::new()));
        let upstream_capture = Arc::clone(&captured);
        let app = Router::new().route(
            "/v1/messages",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let upstream_capture = Arc::clone(&upstream_capture);
                async move {
                    upstream_capture.lock().await.push((
                        body,
                        headers.contains_key(DUCC_HEADER_NAME),
                        headers.contains_key("x-api-key")
                            || headers.contains_key(header::AUTHORIZATION),
                        headers
                            .get("x-hash-key")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_owned),
                    ));
                    Json(json!({"ok":true}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bridge = DuccBridge::spawn(reqwest::Client::new()).await.unwrap();
        let expected = json!({
            "model":"DeepSeek-V4-Flash",
            "stream":true,
            "system":[{"type":"text","text":"only caller instructions"}],
            "messages":[{
                "role":"user",
                "content":[
                    {"type":"text","text":"inspect image"},
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}
                ]
            }],
            "tools":[{"name":"declared_tool","description":"declared","input_schema":{"type":"object"}}]
        });
        let mut extra = HeaderMap::new();
        extra.insert("x-hash-key", "session-hash".parse().unwrap());
        // This must not survive into the forwarded request.
        extra.insert(header::CONTENT_LENGTH, "0".parse().unwrap());
        let (marker, response) = bridge
            .register(
                "DeepSeek-V4-Flash",
                format!("http://{address}/v1/messages").parse().unwrap(),
                expected.clone(),
                extra,
                1,
            )
            .await
            .unwrap();
        let trigger = json!({
            "model":"DeepSeek-V4-Flash",
            "system":"DUCC additions",
            "messages":[{"role":"user","content":marker}],
            "tools":[{"name":"DUCC_only_tool"}]
        });
        let url = format!("{}/v1/messages?beta=true", bridge.base_url);
        let client = reqwest::Client::new();
        // The fresh-process helper call matches the same model and marker but
        // is locally satisfied without consuming the user policy.
        let auxiliary = client
            .post(&url)
            .header(DUCC_HEADER_NAME, "native-value")
            .header("x-api-key", LOOPBACK_API_KEY)
            .json(&trigger)
            .send()
            .await
            .unwrap();
        assert!(auxiliary.status().is_success());
        assert!(captured.lock().await.is_empty());

        let main = client
            .post(&url)
            .header(DUCC_HEADER_NAME, "native-value")
            .header("x-api-key", LOOPBACK_API_KEY)
            .json(&trigger)
            .send()
            .await
            .unwrap();
        assert!(main.status().is_success());
        assert!(
            tokio::time::timeout(Duration::from_secs(1), response)
                .await
                .unwrap()
                .unwrap()
                .status()
                .is_success()
        );
        let snapshot = captured.lock().await;
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, expected);
        assert!(snapshot[0].1);
        assert!(!snapshot[0].2, "loopback placeholder auth must be removed");
        assert_eq!(snapshot[0].3.as_deref(), Some("session-hash"));
        drop(snapshot);

        // Replaying the same DUCC request is answered locally and never opens
        // a second upstream inference.
        let replay = client
            .post(&url)
            .header(DUCC_HEADER_NAME, "native-value")
            .json(&trigger)
            .send()
            .await
            .unwrap();
        assert!(replay.status().is_success());
        assert_eq!(captured.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn missing_native_ducc_header_consumes_policy_and_fails_closed() {
        let bridge = DuccBridge::spawn(reqwest::Client::new()).await.unwrap();
        let expected = json!({"model":"Claude Sonnet 5","stream":true,"messages":[]});
        let (marker, response) = bridge
            .register(
                "Claude Sonnet 5",
                "http://127.0.0.1:9/v1/messages".parse().unwrap(),
                expected,
                HeaderMap::new(),
                0,
            )
            .await
            .unwrap();
        let trigger = json!({
            "model":"Claude Sonnet 5",
            "messages":[{"role":"user","content":marker}]
        });
        let client = reqwest::Client::new();
        let rejected = client
            .post(format!("{}/v1/messages", bridge.base_url))
            .json(&trigger)
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_GATEWAY);
        assert!(response.await.is_err());

        let replay = client
            .post(format!("{}/v1/messages", bridge.base_url))
            .header(DUCC_HEADER_NAME, "late-native-value")
            .json(&trigger)
            .send()
            .await
            .unwrap();
        assert!(replay.status().is_success());
    }
}
