use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode, header};
use futures_util::{FutureExt, SinkExt, StreamExt};
use ring::digest::{Digest, SHA256};
use serde_json::{Value, json};
use tokio_tungstenite_proxy::tungstenite::client::IntoClientRequest;
use tokio_tungstenite_proxy::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite_proxy::tungstenite::{Error as WebSocketError, Message};
use tokio_tungstenite_proxy::{MaybeTlsStream, WebSocketStream};

use super::UpstreamAccess;
use super::official::FORWARDED_OFFICIAL_HEADERS;
use super::websocket::connect_official_websocket;
use crate::error::GatewayError;
use crate::protocol::sse::{
    encode_raw_event, response_failed_payload, response_failed_payload_with_error,
};

const MAX_IDLE_SOCKETS: usize = 16;
const SOCKET_IDLE_LIMIT: Duration = Duration::from_secs(60);
const SOCKET_PROBE_AFTER: Duration = Duration::from_secs(10);
const SOCKET_PROBE_TIMEOUT: Duration = Duration::from_millis(750);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const UNSUPPORTED_COOLDOWN: Duration = Duration::from_secs(300);
const HANDSHAKE_COOLDOWN: Duration = Duration::from_secs(15);
const MAX_HISTORY_ITEMS: usize = 16_384;
const RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
const TURN_METADATA_HEADERS: [(&str, &str); 5] = [
    ("x-codex-turn-state", "x-codex-turn-state"),
    ("x-codex-turn-metadata", "x-codex-turn-metadata"),
    ("x-client-request-id", "x-client-request-id"),
    ("x-request-id", "x-request-id"),
    (
        "x-openai-internal-codex-responses-lite",
        "ws_request_header_x_openai_internal_codex_responses_lite",
    ),
];

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Default)]
pub(super) struct ResponsesWebSockets {
    idle: VecDeque<IdleSocket>,
    unsupported_until: Option<Instant>,
}

struct IdleSocket {
    key: Digest,
    socket: Socket,
    idle_since: Instant,
    fields: Digest,
    prefix: Vec<[u8; 32]>,
    response_id: String,
}

impl UpstreamAccess {
    pub(super) async fn try_official_responses_websocket(
        &self,
        mut url: reqwest::Url,
        headers: &HeaderMap,
        (authorization, account_id): (&HeaderValue, &HeaderValue),
        mut body: Value,
    ) -> Result<Option<reqwest::Response>, GatewayError> {
        let Some(key) = socket_key(headers, authorization, account_id) else {
            return Ok(None);
        };
        let scheme = match url.scheme() {
            "https" => "wss",
            "http" => "ws",
            _ => return Ok(None),
        };
        if url.set_scheme(scheme).is_err()
            || !body["input"]
                .as_array()
                .is_some_and(|items| items.len() <= MAX_HISTORY_ITEMS)
            || headers.contains_key("x-oai-attestation")
        {
            return Ok(None);
        }
        let idle = {
            let mut pool = self.responses_websockets.lock().await;
            if pool
                .unsupported_until
                .is_some_and(|until| until > Instant::now())
            {
                return Ok(None);
            }
            pool.idle
                .retain(|entry| entry.idle_since.elapsed() < SOCKET_IDLE_LIMIT);
            let position = pool
                .idle
                .iter()
                .position(|entry| entry.key.as_ref() == key.as_ref());
            position.and_then(|position| pool.idle.remove(position))
        };
        let idle = live_idle_socket(idle).await;
        let (fields, mut items) = request_fingerprint(&mut body)?;
        let historical_items = idle
            .as_ref()
            .filter(|entry| entry.fields.as_ref() == fields.as_ref())
            .and_then(|entry| items.strip_prefix(entry.prefix.as_slice()))
            .filter(|new_items| !new_items.is_empty())
            .map_or(0, |new_items| items.len() - new_items.len());
        attach_turn_metadata(headers, &mut body);
        body["type"] = json!("response.create");
        let socket = match (idle, historical_items) {
            (Some(entry), 0) => Some(entry.socket),
            (Some(entry), historical_items) => {
                let input = body["input"]
                    .as_array_mut()
                    .context("missing Responses input")
                    .map_err(GatewayError::Other)?;
                input.drain(..historical_items);
                body["previous_response_id"] = json!(entry.response_id);
                Some(entry.socket)
            }
            (None, _) => {
                self.connect_responses_websocket(url, headers, authorization, account_id)
                    .await?
            }
        };
        let Some(mut socket) = socket else {
            return Ok(None);
        };
        let timeout = self.config.request_timeout;
        let message = body.to_string();
        tracing::debug!(
            request_bytes = message.len(),
            historical_items,
            "sending official Responses request over WebSocket"
        );
        // A transport error after a write may mean the server accepted the turn.
        // Only explicit pre-generation rejections are eligible for an HTTP replay.
        tokio::time::timeout(timeout, socket.send(Message::Text(message.into())))
            .await
            .context("official Responses WebSocket write timed out")
            .and_then(|result| result.context("writing official Responses WebSocket request"))
            .map_err(GatewayError::Other)?;
        let Some(first_event) = read_event(&mut socket, timeout)
            .await
            .map_err(GatewayError::Other)?
        else {
            tracing::info!("official Responses WebSocket refused the message size; using HTTP");
            return Ok(None);
        };
        let code = first_event["error"]["code"].as_str();
        if first_event["type"] == "error"
            && (code == Some("websocket_connection_limit_reached")
                || (historical_items > 0 && code == Some("previous_response_not_found")))
        {
            tracing::info!(
                ?code,
                "official Responses WebSocket rejected continuation; using HTTP"
            );
            return Ok(None);
        }
        if first_event["type"] == "error"
            && let Some(status) = first_event["status"]
                .as_u64()
                .and_then(|status| u16::try_from(status).ok())
                .and_then(|status| StatusCode::from_u16(status).ok())
                .filter(|status| status.is_client_error() || status.is_server_error())
        {
            let response = Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(json!({"error": first_event["error"]}).to_string())
                .map_err(|error| GatewayError::Other(error.into()))?;
            return Ok(Some(reqwest::Response::from(response)));
        }
        let pool = self.responses_websockets.clone();
        let model = body.get("model").and_then(Value::as_str).map(str::to_owned);
        let stream = async_stream::stream! {
            let mut event = first_event;
            let mut response_id = None;
            let mut emitted_items = Some(Vec::new());
            loop {
                let kind = event["type"].as_str().unwrap_or_default();
                if kind == "error" {
                    let error = event.get("error").filter(|error| error.is_object()).cloned()
                        .unwrap_or_else(|| json!({"type":"server_error","message":"official Responses WebSocket error"}));
                    let failure = response_failed_payload_with_error(response_id, model.as_deref(), error);
                    yield Ok::<_, Infallible>(encode_raw_event("response.failed", &failure.to_string()));
                    break;
                }
                if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                    response_id = Some(id.to_owned());
                }
                if kind == "response.output_item.done" && let Some(items) = &mut emitted_items {
                    if event["output_index"].as_u64() == Some(items.len() as u64)
                        && items.len() < MAX_HISTORY_ITEMS
                        && let Some(item) = event.get("item")
                    {
                        items.push(item_fingerprint(item));
                    } else {
                        emitted_items = None;
                    }
                }
                if kind == "response.completed" {
                    let bytes = encode_raw_event(kind, &event.to_string());
                    if let Some(completed_items) = completed_output_fingerprints(&event, emitted_items)
                        && items.len() + completed_items.len() <= MAX_HISTORY_ITEMS
                        && let Some(id) = response_id.filter(|id| !id.is_empty())
                    {
                        items.extend(completed_items);
                        let mut pool = pool.lock().await;
                        pool.idle.retain(|entry| entry.key.as_ref() != key.as_ref());
                        pool.idle.push_back(IdleSocket {
                            key,
                            socket,
                            idle_since: Instant::now(),
                            fields,
                            prefix: items,
                            response_id: id,
                        });
                        if pool.idle.len() > MAX_IDLE_SOCKETS {
                            pool.idle.pop_front();
                        }
                    }
                    yield Ok(bytes);
                    break;
                }
                yield Ok(encode_raw_event(kind, &event.to_string()));
                if matches!(kind, "response.failed" | "response.incomplete") {
                    break;
                }
                match read_event(&mut socket, timeout).await {
                    Ok(Some(next)) => event = next,
                    failure => {
                        let error = failure.err();
                        tracing::warn!(?error, "official Responses WebSocket ended after the response started");
                        let event = response_failed_payload(
                            response_id, model.as_deref(),
                            "Official Responses WebSocket closed before the turn finished",
                            "server_error",
                        );
                        yield Ok(encode_raw_event("response.failed", &event.to_string()));
                        break;
                    }
                }
            }
        };
        let response = Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(reqwest::Body::wrap_stream(stream))
            .map_err(|error| GatewayError::Other(error.into()))?;
        Ok(Some(reqwest::Response::from(response)))
    }

    async fn connect_responses_websocket(
        &self,
        url: reqwest::Url,
        incoming: &HeaderMap,
        authorization: &HeaderValue,
        account_id: &HeaderValue,
    ) -> Result<Option<Socket>, GatewayError> {
        let mut request = url
            .as_str()
            .into_client_request()
            .context("building official Responses WebSocket request")
            .map_err(GatewayError::Other)?;
        let headers = request.headers_mut();
        for &name in FORWARDED_OFFICIAL_HEADERS {
            if let Some(value) = incoming.get(name) {
                headers.insert(name, value.clone());
            }
        }
        for (name, _) in TURN_METADATA_HEADERS {
            headers.remove(name);
        }
        headers.insert(header::AUTHORIZATION, authorization.clone());
        headers.insert("chatgpt-account-id", account_id.clone());
        headers.insert(
            "openai-beta",
            HeaderValue::from_static(RESPONSES_WEBSOCKET_BETA),
        );
        let result = tokio::time::timeout(
            HANDSHAKE_TIMEOUT.min(self.config.request_timeout),
            connect_official_websocket(request, self, &self.proxy_selection),
        )
        .await;
        match result {
            Ok(Ok(socket)) => Ok(Some(socket)),
            Ok(Err(error)) => {
                let status = error
                    .downcast_ref::<WebSocketError>()
                    .and_then(|error| match error {
                        WebSocketError::Http(response) => Some(response.status()),
                        _ => None,
                    });
                let cooldown = match status {
                    Some(StatusCode::UNAUTHORIZED | StatusCode::TOO_MANY_REQUESTS) => None,
                    Some(
                        StatusCode::NOT_FOUND
                        | StatusCode::METHOD_NOT_ALLOWED
                        | StatusCode::UPGRADE_REQUIRED,
                    ) => Some(UNSUPPORTED_COOLDOWN),
                    _ => Some(HANDSHAKE_COOLDOWN),
                };
                if let Some(cooldown) = cooldown {
                    self.responses_websockets.lock().await.unsupported_until =
                        Some(Instant::now() + cooldown);
                }
                tracing::info!(
                    ?status,
                    "official Responses WebSocket handshake failed; using HTTP"
                );
                Ok(None)
            }
            Err(_) => {
                self.responses_websockets.lock().await.unsupported_until =
                    Some(Instant::now() + HANDSHAKE_COOLDOWN);
                tracing::info!("official Responses WebSocket handshake timed out; using HTTP");
                Ok(None)
            }
        }
    }
}

fn socket_key(
    headers: &HeaderMap,
    authorization: &HeaderValue,
    account_id: &HeaderValue,
) -> Option<Digest> {
    headers
        .get("thread-id")
        .or_else(|| headers.get("session-id"))
        .or_else(|| headers.get("x-session-id"))
        .filter(|value| !value.is_empty())?;
    let mut key = ring::digest::Context::new(&SHA256);
    let stable_headers = FORWARDED_OFFICIAL_HEADERS
        .iter()
        .copied()
        .filter(|name| !TURN_METADATA_HEADERS.iter().any(|(turn, _)| name == turn))
        .chain(["authorization", "x-api-key"])
        .map(|name| headers.get(name));
    for value in [Some(authorization), Some(account_id)]
        .into_iter()
        .chain(stable_headers)
    {
        let bytes = value.map_or(&[][..], HeaderValue::as_bytes);
        key.update(&bytes.len().to_le_bytes());
        key.update(bytes);
    }
    Some(key.finish())
}

fn request_fingerprint(body: &mut Value) -> Result<(Digest, Vec<[u8; 32]>), GatewayError> {
    let body = body
        .as_object_mut()
        .context("invalid Responses request body")
        .map_err(GatewayError::Other)?;
    let input = body.remove("input").unwrap_or_default();
    let items = input
        .as_array()
        .context("missing Responses input")
        .map_err(GatewayError::Other)?;
    let fingerprints = items.iter().map(item_fingerprint).collect();
    // Turn metadata is refreshed on the existing socket. Every other option
    // must agree before the previous response can stand in for the prefix.
    let metadata = body.remove("client_metadata");
    let encoded = serde_json::to_vec(body).map_err(|error| GatewayError::Other(error.into()))?;
    let fields = ring::digest::digest(&SHA256, &encoded);
    if let Some(metadata) = metadata {
        body.insert("client_metadata".to_owned(), metadata);
    }
    body.insert("input".to_owned(), input);
    Ok((fields, fingerprints))
}

fn item_fingerprint(item: &Value) -> [u8; 32] {
    let mut fingerprint = [0; 32];
    fingerprint
        .copy_from_slice(ring::digest::digest(&SHA256, item.to_string().as_bytes()).as_ref());
    fingerprint
}

fn completed_output_fingerprints(
    event: &Value,
    emitted: Option<Vec<[u8; 32]>>,
) -> Option<Vec<[u8; 32]>> {
    let emitted = emitted?;
    let terminal = event.pointer("/response/output").and_then(Value::as_array);
    // A completed event may omit output already sent by output_item.done.
    if terminal.is_none_or(Vec::is_empty) {
        return (terminal.is_some() || !emitted.is_empty()).then_some(emitted);
    }
    let terminal = terminal.filter(|items| items.len() <= MAX_HISTORY_ITEMS)?;
    let terminal: Vec<_> = terminal.iter().map(item_fingerprint).collect();
    (emitted.is_empty() || emitted == terminal).then_some(terminal)
}

fn attach_turn_metadata(headers: &HeaderMap, body: &mut Value) {
    for (header, field) in TURN_METADATA_HEADERS {
        if let Some(value) = headers.get(header).and_then(|value| value.to_str().ok()) {
            if !body["client_metadata"].is_object() {
                body["client_metadata"] = json!({});
            }
            body["client_metadata"][field] = json!(value);
        }
    }
}

async fn live_idle_socket(entry: Option<IdleSocket>) -> Option<IdleSocket> {
    let mut entry = entry?;
    // A peer can close an idle socket before the next HTTP turn arrives.
    // Discard an already-readable close before writing a new turn.
    if matches!(
        entry.socket.next().now_or_never(),
        Some(None | Some(Err(_) | Ok(Message::Close(_))))
    ) {
        return None;
    }
    if entry.idle_since.elapsed() < SOCKET_PROBE_AFTER {
        return Some(entry);
    }
    let is_live = tokio::time::timeout(SOCKET_PROBE_TIMEOUT, async {
        let nonce = uuid::Uuid::new_v4();
        if entry
            .socket
            .send(Message::Ping(nonce.as_bytes().to_vec().into()))
            .await
            .is_err()
        {
            return false;
        }
        while let Some(Ok(message)) = entry.socket.next().await {
            match message {
                Message::Pong(payload) if payload.as_ref() == nonce.as_bytes() => return true,
                Message::Close(_) => return false,
                _ => {}
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    is_live.then_some(entry)
}

// A 1009 close explicitly rejects the message. Other transport failures
// cannot establish that the turn is safe to send again over HTTP.
async fn read_event(socket: &mut Socket, timeout: Duration) -> anyhow::Result<Option<Value>> {
    tokio::time::timeout(timeout, async {
        while let Some(message) = socket.next().await {
            let bytes = match message.context("reading official Responses WebSocket")? {
                Message::Text(text) => bytes::Bytes::from(text),
                Message::Binary(bytes) => bytes,
                Message::Ping(_) => {
                    socket.flush().await?;
                    continue;
                }
                Message::Close(Some(frame)) if frame.code == CloseCode::Size => return Ok(None),
                Message::Close(_) => break,
                _ => continue,
            };
            let event: Value = serde_json::from_slice(&bytes)
                .context("invalid official Responses WebSocket event")?;
            if event["type"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("response.") || kind == "error")
            {
                return Ok(Some(event));
            }
        }
        anyhow::bail!("official Responses WebSocket closed before a terminal event")
    })
    .await
    .context("idle timeout waiting for official Responses WebSocket event")?
}
