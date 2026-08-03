use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message as AxumWsMessage, WebSocketUpgrade};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use codex_mixin::config::{GatewayConfig, ThinkingMode};
use codex_mixin::provider::{ProviderModel, custom_provider};
use codex_mixin::server::{AppState, router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[derive(Clone)]
struct OfficialState {
    connections: Arc<AtomicUsize>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_websocket_uses_http_proxy() {
    let (official_url, official_connections) = spawn_official_mock().await;
    let (proxy_url, proxy_connects) = spawn_connect_proxy().await;
    let proxy_url_for_env = proxy_url.clone();
    let (gateway_url, _codex_home) = spawn_gateway_with_env(official_url, move |name| match name {
        "HTTP_PROXY" | "HTTPS_PROXY" => Some(proxy_url_for_env.clone()),
        _ => None,
    })
    .await;

    let frames = complete_official_ws_turn(&gateway_url).await;

    assert!(
        frames
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );
    assert_eq!(official_connections.load(Ordering::SeqCst), 1);
    assert_eq!(proxy_connects.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_websocket_honors_no_proxy() {
    let (official_url, official_connections) = spawn_official_mock().await;
    let (proxy_url, proxy_connects) = spawn_connect_proxy().await;
    let proxy_url_for_env = proxy_url.clone();
    let (gateway_url, _codex_home) = spawn_gateway_with_env(official_url, move |name| match name {
        "HTTP_PROXY" => Some(proxy_url_for_env.clone()),
        "NO_PROXY" => Some("127.0.0.1".to_owned()),
        _ => None,
    })
    .await;

    let frames = complete_official_ws_turn(&gateway_url).await;

    assert!(
        frames
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );
    assert_eq!(official_connections.load(Ordering::SeqCst), 1);
    assert_eq!(proxy_connects.load(Ordering::SeqCst), 0);
}

async fn spawn_official_mock() -> (String, Arc<AtomicUsize>) {
    let connections = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/v1/responses", get(official_responses_ws))
        .with_state(OfficialState {
            connections: connections.clone(),
        });
    (spawn_router(app).await, connections)
}

async fn official_responses_ws(
    State(state): State<OfficialState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        state.connections.fetch_add(1, Ordering::SeqCst);
        while let Some(Ok(message)) = socket.next().await {
            match message {
                AxumWsMessage::Text(_) | AxumWsMessage::Binary(_) => {
                    for payload in [
                        json!({
                            "type": "response.created",
                            "response": {
                                "id": "resp_proxy_test",
                                "object": "response",
                                "status": "in_progress",
                                "output": []
                            }
                        }),
                        json!({
                            "type": "response.completed",
                            "response": {
                                "id": "resp_proxy_test",
                                "object": "response",
                                "status": "completed",
                                "output": []
                            }
                        }),
                    ] {
                        socket
                            .send(AxumWsMessage::Text(payload.to_string().into()))
                            .await
                            .unwrap();
                    }
                }
                AxumWsMessage::Ping(bytes) => {
                    socket.send(AxumWsMessage::Pong(bytes)).await.unwrap();
                }
                AxumWsMessage::Pong(_) => {}
                AxumWsMessage::Close(_) => break,
            }
        }
    })
    .into_response()
}

async fn spawn_connect_proxy() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connects = Arc::new(AtomicUsize::new(0));
    let connects_for_loop = connects.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else {
                break;
            };
            let connects = connects_for_loop.clone();
            tokio::spawn(async move {
                let Some(target) = read_connect_target(&mut client).await else {
                    return;
                };
                connects.fetch_add(1, Ordering::SeqCst);
                if let Ok(mut upstream) = TcpStream::connect(&target).await {
                    let _ = client
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await;
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                } else {
                    let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                }
            });
        }
    });
    (format!("http://{addr}"), connects)
}

async fn read_connect_target(stream: &mut TcpStream) -> Option<String> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 2048];
    loop {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        request.extend_from_slice(&buffer[..read]);
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..end]);
            let mut parts = head.split_whitespace();
            if parts.next()? == "CONNECT" {
                return parts.next().map(str::to_owned);
            }
            return None;
        }
        if request.len() > 8192 {
            return None;
        }
    }
}

async fn spawn_gateway_with_env(
    official_url: String,
    env_lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) -> (String, tempfile::TempDir) {
    let codex_home = tempfile::tempdir().unwrap();
    let auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let config = test_config(format!("{official_url}/v1/responses"), auth_path);
    let state = AppState::with_env_lookup(config, env_lookup).unwrap();
    (spawn_router(router(state)).await, codex_home)
}

fn test_config(official_responses_url: String, codex_auth_path: PathBuf) -> GatewayConfig {
    let mut provider = custom_provider("custom", "upstream-key");
    provider.base_url = "http://127.0.0.1:1".to_owned();
    provider.cached_models = vec![ProviderModel {
        id: "custom-model".to_owned(),
        ..ProviderModel::default()
    }];
    provider.selected_models = vec!["custom-model".to_owned()];
    GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![provider],
        official_responses_url,
        codex_auth_path,
        gateway_api_key: Some("gateway-key".to_owned()),
        accept_codex_oauth: true,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(10),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    }
}

async fn spawn_router(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn official_ws_request() -> Value {
    json!({
        "type": "response.create",
        "model": "gpt-5.5",
        "stream": true,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "hello"}]
        }]
    })
}

async fn complete_official_ws_turn(gateway_url: &str) -> Vec<String> {
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(WsMessage::Text(official_ws_request().to_string().into()))
        .await
        .unwrap();
    websocket_response_frames(&mut socket).await
}

async fn websocket_response_frames(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
) -> Vec<String> {
    let mut frames = Vec::new();
    while let Some(message) = socket.next().await {
        match message.unwrap() {
            WsMessage::Text(text) => {
                let text = text.to_string();
                let terminal = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|event| event.get("type").and_then(Value::as_str).map(str::to_owned))
                    .is_some_and(|event_type| {
                        matches!(
                            event_type.as_str(),
                            "response.completed"
                                | "response.failed"
                                | "response.incomplete"
                                | "error"
                        )
                    });
                frames.push(text);
                if terminal {
                    break;
                }
            }
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    frames
}
