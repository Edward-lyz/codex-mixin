use std::time::Duration;

use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use axum::http::{HeaderMap, header};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use memchr::memmem;
use serde_json::{Value, json};
use tokio_tungstenite_proxy::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite_proxy::tungstenite::client::IntoClientRequest;

use super::super::AppState;
use super::{
    OfficialWebSocket, ResponsesWsContext, take_custom_request_input, tungstenite_to_axum_message,
};
use crate::protocol::convert::collaboration::{self, NAMESPACE as COLLABORATION_NAMESPACE};
use crate::upstream::FORWARDED_OFFICIAL_HEADERS;
use crate::upstream::websocket::connect_official_websocket;

#[derive(Debug)]
struct OfficialWebSocketRequestError {
    source: anyhow::Error,
    response_started: bool,
    response_id: Option<String>,
}

#[derive(Debug)]
pub(super) struct OfficialWebSocketState {
    response_id: String,
    model: String,
    pub(super) history: Vec<Value>,
    plaintext_collaboration: bool,
    collaboration_tool: Option<Value>,
}

#[derive(Debug)]
enum OfficialWebSocketResponse {
    Completed {
        response_id: String,
        items_added: Vec<Value>,
    },
    Failed,
}

pub(super) async fn proxy_official_ws_request(
    context: &mut ResponsesWsContext<'_>,
    body: &mut Value,
    model: &str,
    request_history: &mut Option<OfficialWebSocketState>,
    usage_observer: &mut Option<crate::gateway::UpstreamCacheObserver>,
) -> anyhow::Result<Option<(anyhow::Error, Option<String>)>> {
    let mut retry_available = true;
    loop {
        if context.official_socket.is_none()
            && let Err(error) = connect_and_expand_official_request(
                context.state,
                context.headers,
                context.official_socket,
                body,
                request_history,
            )
            .await
        {
            if retry_available {
                retry_available = false;
                tracing::warn!(model, error = %error, "retrying official responses websocket connection");
                continue;
            }
            return Ok(Some((error, None)));
        }
        let inherited_tools = request_history
            .as_ref()
            .filter(|previous| previous.plaintext_collaboration)
            .map(|previous| {
                (
                    previous.history.as_slice(),
                    previous.collaboration_tool.as_ref(),
                )
            });
        let saved = collaboration::prepare_request(body, inherited_tools)?;
        let plaintext_collaboration = saved.active();
        let result = proxy_official_responses_ws(
            context
                .official_socket
                .as_mut()
                .expect("official websocket connected"),
            context.client_sender,
            body,
            context.state.config.request_timeout,
            usage_observer.as_mut(),
            plaintext_collaboration,
        )
        .await;
        collaboration::restore_request(body, saved);
        match result {
            Ok(OfficialWebSocketResponse::Completed {
                response_id,
                items_added,
            }) => {
                let (mut history, previous_tool) = match request_history.take() {
                    Some(previous) => (previous.history, previous.collaboration_tool),
                    None => (take_custom_request_input(body)?, None),
                };
                history.extend(items_added);
                let collaboration_tool = body["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|tool| {
                        tool["type"] == "namespace" && tool["name"] == COLLABORATION_NAMESPACE
                    })
                    .cloned()
                    .or(previous_tool);
                *context.official_state = Some(OfficialWebSocketState {
                    response_id,
                    model: model.to_owned(),
                    history,
                    plaintext_collaboration,
                    collaboration_tool,
                });
                return Ok(None);
            }
            Ok(OfficialWebSocketResponse::Failed) => {
                *context.official_socket = None;
                *context.official_state = None;
                return Ok(None);
            }
            Err(error) if !error.response_started && retry_available => {
                retry_available = false;
                *context.official_socket = None;
                tracing::warn!(
                    model,
                    error = %error.source,
                    "retrying official responses websocket before any response started"
                );
            }
            Err(error) => {
                *context.official_socket = None;
                return Ok(Some((error.source, error.response_id)));
            }
        }
    }
}

async fn connect_and_expand_official_request(
    state: &AppState,
    headers: &HeaderMap,
    official_socket: &mut Option<OfficialWebSocket>,
    body: &mut Value,
    request_history: &mut Option<OfficialWebSocketState>,
) -> anyhow::Result<()> {
    *official_socket = Some(connect_official_responses_ws(state, headers).await?);
    if let Some(previous) = request_history.take() {
        body["input"] = Value::Array(previous.history);
        body.as_object_mut()
            .expect("responses request is an object")
            .remove("previous_response_id");
        // Dynamic declarations are in the replayed input. On reconnect the
        // original top-level declaration must also be sent to the new socket.
        if let Some(tool) = previous.collaboration_tool {
            if body["tools"].is_null() {
                body["tools"] = json!([]);
            }
            if let Some(tools) = body["tools"].as_array_mut()
                && !tools.iter().any(|tool| {
                    tool["type"] == "namespace" && tool["name"] == COLLABORATION_NAMESPACE
                })
            {
                tools.push(tool);
            }
        }
    }
    Ok(())
}

async fn connect_official_responses_ws(
    state: &AppState,
    headers: &HeaderMap,
) -> anyhow::Result<OfficialWebSocket> {
    let websocket_url = websocket_url_from_http_url(&state.config.official_responses_url)?;
    let mut request = websocket_url.into_client_request()?;
    {
        let request_headers = request.headers_mut();
        let (authorization, account_id) = state.upstream.official_auth().await?;
        request_headers.insert(header::AUTHORIZATION, authorization);
        request_headers.insert("chatgpt-account-id", account_id);
        for &name in FORWARDED_OFFICIAL_HEADERS {
            if let Some(value) = headers.get(name) {
                request_headers.insert(name, value.clone());
            }
        }
    }
    let official_socket = tokio::time::timeout(
        state.config.request_timeout,
        connect_official_websocket(request, &state.upstream, state.websocket_proxy_env()),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "official websocket connect timed out after {:?}",
            state.config.request_timeout
        )
    })??;
    Ok(official_socket)
}

async fn proxy_official_responses_ws(
    official_socket: &mut OfficialWebSocket,
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    body: &Value,
    idle_timeout: Duration,
    mut usage_observer: Option<&mut crate::gateway::UpstreamCacheObserver>,
    plaintext_collaboration: bool,
) -> Result<OfficialWebSocketResponse, OfficialWebSocketRequestError> {
    tokio::time::timeout(
        idle_timeout,
        official_socket.send(TungsteniteMessage::Text(body.to_string().into())),
    )
    .await
    .map_err(|_| OfficialWebSocketRequestError {
        source: anyhow::anyhow!(
            "idle timeout sending official websocket request after {idle_timeout:?}"
        ),
        response_started: false,
        response_id: None,
    })?
    .map_err(|err| OfficialWebSocketRequestError {
        source: err.into(),
        response_started: false,
        response_id: None,
    })?;
    let mut response_started = false;
    let mut response_id = None;
    let mut items_added = Vec::new();
    loop {
        let mut message = tokio::time::timeout(idle_timeout, official_socket.next())
            .await
            .map_err(|_| OfficialWebSocketRequestError {
                source: anyhow::anyhow!(
                    "idle timeout waiting for official websocket after {idle_timeout:?}"
                ),
                response_started,
                response_id: response_id.clone(),
            })?
            .ok_or_else(|| OfficialWebSocketRequestError {
                source: anyhow::anyhow!(
                    "official responses websocket ended before a terminal response"
                ),
                response_started,
                response_id: response_id.clone(),
            })?
            .map_err(|err| OfficialWebSocketRequestError {
                source: err.into(),
                response_started,
                response_id: response_id.clone(),
            })?;
        let mut event = match &message {
            TungsteniteMessage::Text(text) => {
                parse_official_ws_event(text.as_bytes(), response_id.as_deref())
            }
            TungsteniteMessage::Binary(bytes) => {
                parse_official_ws_event(bytes, response_id.as_deref())
            }
            _ => None,
        };
        if plaintext_collaboration && let Some(event) = event.as_mut() {
            collaboration::restore_event(event).map_err(|error| {
                OfficialWebSocketRequestError {
                    source: error.into(),
                    // A protocol rejection must never replay executable calls.
                    response_started: true,
                    response_id: response_id.clone(),
                }
            })?;
            message = TungsteniteMessage::Text(event.to_string().into());
        }
        if let Some(event) = event.as_ref()
            && let Some(observer) = usage_observer.as_deref_mut()
        {
            observer.observe_value(event);
        }
        if response_id.is_none() {
            response_id = event
                .as_ref()
                .and_then(|event| event.pointer("/response/id"))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if event
            .as_ref()
            .and_then(|event| event.get("type"))
            .and_then(Value::as_str)
            == Some("response.output_item.done")
        {
            let item = event
                .as_ref()
                .and_then(|event| event.get("item"))
                .ok_or_else(|| OfficialWebSocketRequestError {
                    source: anyhow::anyhow!("official output_item.done event is missing item"),
                    response_started,
                    response_id: response_id.clone(),
                })?;
            items_added.push(item.clone());
        }
        let terminal_type = event
            .as_ref()
            .and_then(|event| event.get("type"))
            .and_then(Value::as_str)
            .filter(|event_type| {
                matches!(
                    *event_type,
                    "response.completed" | "response.failed" | "response.incomplete" | "error"
                )
            })
            .map(str::to_owned);
        match message {
            TungsteniteMessage::Ping(bytes) => {
                tokio::time::timeout(
                    idle_timeout,
                    official_socket.send(TungsteniteMessage::Pong(bytes)),
                )
                .await
                .map_err(|_| OfficialWebSocketRequestError {
                    source: anyhow::anyhow!(
                        "idle timeout sending official websocket pong after {idle_timeout:?}"
                    ),
                    response_started,
                    response_id: response_id.clone(),
                })?
                .map_err(|err| OfficialWebSocketRequestError {
                    source: err.into(),
                    response_started,
                    response_id: response_id.clone(),
                })?;
            }
            TungsteniteMessage::Pong(_) | TungsteniteMessage::Frame(_) => {}
            TungsteniteMessage::Close(_) => {
                return Err(OfficialWebSocketRequestError {
                    source: anyhow::anyhow!(
                        "official responses websocket closed before a terminal response"
                    ),
                    response_started,
                    response_id,
                });
            }
            message => {
                let client_message = if terminal_type.as_deref() == Some("error") {
                    let error = official_websocket_error(event.as_ref());
                    Some(AxumWsMessage::Text(
                        crate::protocol::sse::response_failed_payload_with_error(
                            response_id.clone(),
                            body.get("model").and_then(Value::as_str),
                            error,
                        )
                        .to_string()
                        .into(),
                    ))
                } else {
                    tungstenite_to_axum_message(message)
                };
                if let Some(message) = client_message {
                    response_started = true;
                    client_sender.send(message).await.map_err(|err| {
                        OfficialWebSocketRequestError {
                            source: err.into(),
                            response_started,
                            response_id: response_id.clone(),
                        }
                    })?;
                }
            }
        }
        if let Some(terminal_type) = terminal_type {
            if terminal_type != "response.completed" {
                return Ok(OfficialWebSocketResponse::Failed);
            }
            let response = event
                .as_ref()
                .and_then(|event| event.get("response"))
                .ok_or_else(|| OfficialWebSocketRequestError {
                    source: anyhow::anyhow!("official completed response is missing response"),
                    response_started,
                    response_id: response_id.clone(),
                })?;
            let completed_response_id = response
                .get("id")
                .and_then(Value::as_str)
                .filter(|response_id| !response_id.is_empty())
                .ok_or_else(|| OfficialWebSocketRequestError {
                    source: anyhow::anyhow!("official completed response is missing id"),
                    response_started,
                    response_id: response_id.clone(),
                })?
                .to_owned();
            return Ok(OfficialWebSocketResponse::Completed {
                response_id: completed_response_id,
                items_added,
            });
        }
    }
}

fn official_websocket_error(event: Option<&Value>) -> Value {
    match event.and_then(|event| event.get("error")) {
        Some(error @ Value::Object(_)) => error.clone(),
        Some(Value::String(message)) => json!({"message": message, "type": "server_error"}),
        _ => json!({
            "message": event
                .and_then(|event| event.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("official responses websocket request failed"),
            "type": "server_error"
        }),
    }
}

fn parse_official_ws_event(bytes: &[u8], response_id: Option<&str>) -> Option<Value> {
    if response_id.is_some()
        && memmem::find(bytes, b"response.output_text.delta").is_none()
        && memmem::find(bytes, b"response.reasoning_summary_text.delta").is_none()
        && memmem::find(bytes, b"response.output_item.done").is_none()
        && memmem::find(bytes, b"response.output_item.added").is_none()
        && memmem::find(bytes, b"response.completed").is_none()
        && memmem::find(bytes, b"response.failed").is_none()
        && memmem::find(bytes, b"response.incomplete").is_none()
        && memmem::find(bytes, b"\"type\": \"error\"").is_none()
        && memmem::find(bytes, b"\"type\":\"error\"").is_none()
    {
        return None;
    }
    serde_json::from_slice::<Value>(bytes).ok()
}

pub(super) fn official_websocket_request_history(
    body: &Value,
    state: Option<OfficialWebSocketState>,
) -> anyhow::Result<Option<OfficialWebSocketState>> {
    let incremental_input = body
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("official request input must be an array"))?;
    let Some(previous_response_id) = body.get("previous_response_id").and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let mut state = state.ok_or_else(|| {
        anyhow::anyhow!("unknown official previous_response_id: {previous_response_id}")
    })?;
    if previous_response_id != state.response_id {
        anyhow::bail!("unknown official previous_response_id: {previous_response_id}");
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("official request is missing model"))?;
    if model != state.model {
        anyhow::bail!(
            "official previous_response_id belongs to model {}",
            state.model
        );
    }
    state.history.extend(incremental_input.iter().cloned());
    Ok(Some(state))
}

fn websocket_url_from_http_url(url: &str) -> anyhow::Result<String> {
    if let Some(rest) = url.strip_prefix("https://") {
        return Ok(format!("wss://{rest}"));
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return Ok(format!("ws://{rest}"));
    }
    anyhow::bail!("official responses URL must start with http:// or https://")
}
