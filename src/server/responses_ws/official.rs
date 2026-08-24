use std::time::Duration;

use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use axum::http::{HeaderMap, header};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use memchr::memmem;
use serde_json::Value;
use tokio_tungstenite_proxy::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite_proxy::tungstenite::client::IntoClientRequest;

use super::super::AppState;
use super::super::auth::FORWARDED_OFFICIAL_HEADERS;
use super::super::websocket_proxy::connect_upstream_websocket;
use super::{
    OfficialWebSocket, ResponsesWsContext, take_custom_request_input, tungstenite_to_axum_message,
};

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
    history: Vec<Value>,
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
    request_history: &mut Option<Vec<Value>>,
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
        match proxy_official_responses_ws(
            context
                .official_socket
                .as_mut()
                .expect("official websocket connected"),
            context.client_sender,
            body,
            context.state.config.request_timeout,
            usage_observer.as_mut(),
        )
        .await
        {
            Ok(OfficialWebSocketResponse::Completed {
                response_id,
                items_added,
            }) => {
                let mut history = match request_history.take() {
                    Some(history) => history,
                    None => take_custom_request_input(body)?,
                };
                history.extend(items_added);
                *context.official_state = Some(OfficialWebSocketState {
                    response_id,
                    model: model.to_owned(),
                    history,
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
                    "reconnecting stale official responses websocket"
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
    request_history: &mut Option<Vec<Value>>,
) -> anyhow::Result<()> {
    *official_socket = Some(connect_official_responses_ws(state, headers).await?);
    if body.get("previous_response_id").is_some() {
        body["input"] = Value::Array(
            request_history
                .take()
                .expect("previous response history is available"),
        );
        body.as_object_mut()
            .expect("responses request is an object")
            .remove("previous_response_id");
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
        let (authorization, account_id) = state.official_auth().await?;
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
        connect_upstream_websocket(request, state.websocket_proxy_env()),
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
        let message = tokio::time::timeout(idle_timeout, official_socket.next())
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
        let event = match &message {
            TungsteniteMessage::Text(text) => {
                parse_official_ws_event(text.as_bytes(), response_id.as_deref())
            }
            TungsteniteMessage::Binary(bytes) => {
                parse_official_ws_event(bytes, response_id.as_deref())
            }
            _ => None,
        };
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
                if let Some(message) = tungstenite_to_axum_message(message) {
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

pub(super) fn effective_official_cache_body(
    body: &Value,
    request_history: Option<&[Value]>,
) -> Value {
    let mut effective = body.clone();
    if let Some(request_history) = request_history {
        effective["input"] = Value::Array(request_history.to_vec());
        if let Some(effective) = effective.as_object_mut() {
            effective.remove("previous_response_id");
        }
    }
    effective
}

fn parse_official_ws_event(bytes: &[u8], response_id: Option<&str>) -> Option<Value> {
    if response_id.is_some()
        && memmem::find(bytes, b"response.output_text.delta").is_none()
        && memmem::find(bytes, b"response.reasoning_summary_text.delta").is_none()
        && memmem::find(bytes, b"response.output_item.done").is_none()
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
) -> anyhow::Result<Option<Vec<Value>>> {
    let incremental_input = body
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("official request input must be an array"))?;
    let Some(previous_response_id) = body.get("previous_response_id").and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let state = state.ok_or_else(|| {
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
    let mut history = state.history;
    history.extend(incremental_input.iter().cloned());
    Ok(Some(history))
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_official_ws_event;
    use crate::gateway::{
        CacheShape, CacheShapeTracker, UpstreamCacheObserver, record_provider_prefix,
    };
    use crate::upstream::UpstreamRouting;

    #[test]
    fn official_websocket_records_response_timing_after_response_id() {
        let tracker = CacheShapeTracker::default();
        let request = json!({
            "model": "gpt-5.6-sol",
            "input": [{"type": "message", "role": "user", "content": "hello"}]
        });
        let routing = UpstreamRouting {
            session_id: "thread-1".to_owned(),
            hash_key: "hash-1".to_owned(),
        };
        let observation = record_provider_prefix(
            &tracker,
            "official",
            "gpt-5.6-sol",
            "gpt-5.6-sol",
            Some(&routing),
            CacheShape::from_openai_responses(&request),
        )
        .unwrap();
        let mut observer = UpstreamCacheObserver::new(observation);

        let delta = br#"{"type":"response.output_text.delta","delta":"hi"}"#;
        observer.observe_value(
            &parse_official_ws_event(delta, Some("response-1"))
                .expect("output delta remains visible to timing observer"),
        );
        observer.observe_value(&json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 4,
                    "input_tokens_details": {"cached_tokens": 3},
                    "output_tokens": 2
                }
            }
        }));
        drop(observer);

        let usage = tracker.usage_snapshot();
        assert!(usage[0].average_ttft_ms.is_some());
        assert!(usage[0].output_tps.is_some());
    }
}
