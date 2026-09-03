use axum::extract::ws::{Message as AxumWsMessage, WebSocket};
use axum::http::HeaderMap;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::AppState;
use super::super::auth::stable_oneapi_routing;
use super::{ResponsesWsContext, take_custom_request_input};
use crate::fusion::{FusionEngine, should_fuse_turn};
use crate::gateway::{RequestPlan, ResolvedModelRoute, UpstreamExecutor};
use crate::protocol::sse::SseDecoder;

#[derive(Debug)]
pub(super) struct CustomWebSocketState {
    response_id: String,
    model: String,
    route: ResolvedModelRoute,
    history: Vec<Value>,
}

pub(super) async fn run_custom_ws_request(
    context: &mut ResponsesWsContext<'_>,
    body: &mut Value,
) -> anyhow::Result<Option<CustomWebSocketState>> {
    expand_custom_websocket_history(context.state, body, context.custom_state.take()).await?;
    if is_noop_responses_ws_request(body) {
        return complete_custom_noop(context.state, context.client_sender, body.take())
            .await
            .map(Some);
    }
    strip_custom_websocket_envelope(body);
    proxy_custom_responses_ws(
        context.state,
        context.headers,
        context.client_sender,
        body.take(),
    )
    .await
}

fn is_noop_responses_ws_request(body: &Value) -> bool {
    if body.get("generate").and_then(Value::as_bool) == Some(false) {
        return true;
    }
    body.get("input")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
}

fn strip_custom_websocket_envelope(body: &mut Value) {
    if let Some(body) = body.as_object_mut() {
        body.remove("type");
        body.remove("previous_response_id");
    }
}

async fn proxy_custom_responses_ws(
    state: &AppState,
    headers: &HeaderMap,
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    mut body: Value,
) -> anyhow::Result<Option<CustomWebSocketState>> {
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("custom request is missing model"))?
        .to_owned();
    let route = state
        .resolve_model_route(&requested_model)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let model = requested_model;
    if !body.get("input").is_some_and(Value::is_array) {
        anyhow::bail!("custom request input must be an array");
    }
    let provider_routing = stable_oneapi_routing(headers, &body)?;
    let mut preserved_history = None;
    let (stream, returned_body) = match &route {
        ResolvedModelRoute::Official => {
            anyhow::bail!("official model reached custom websocket proxy")
        }
        ResolvedModelRoute::Provider { .. } => {
            let plan =
                RequestPlan::from_route(route.clone(), body, provider_routing.clone(), None)?;
            let (stream, body) = UpstreamExecutor::new(state)
                .stream_and_return_body(plan, headers)
                .await?;
            (stream, Some(body))
        }
        ResolvedModelRoute::Fusion { profile_id } => {
            let profile = state
                .config
                .fusion_profiles
                .iter()
                .find(|profile| profile.id == *profile_id)
                .ok_or_else(|| anyhow::anyhow!("unknown fusion profile: {profile_id}"))?
                .clone();
            if should_fuse_turn(&body) {
                preserved_history = Some(
                    body.get("input")
                        .and_then(Value::as_array)
                        .cloned()
                        .expect("validated custom request input"),
                );
                let stream = FusionEngine::new(state, &profile)
                    .with_headers(headers.clone())
                    .stream_with_routing(body, provider_routing);
                (stream, None)
            } else {
                body["stream"] = Value::Bool(true);
                let (stream, body) = FusionEngine::new(state, &profile)
                    .with_headers(headers.clone())
                    .stream_final_continuation_and_body(body, provider_routing.as_ref())
                    .await?;
                (stream, Some(body))
            }
        }
    };
    tokio::pin!(stream);
    let mut decoder = SseDecoder::default();
    let mut completed_response = None;
    let mut failed = false;
    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(never) => match never {},
        };
        for event in decoder.push(&bytes) {
            match event.event.as_deref() {
                Some("response.completed") => {
                    let mut payload: Value = serde_json::from_str(&event.data)?;
                    completed_response = payload.get_mut("response").map(Value::take);
                }
                Some("response.failed" | "response.incomplete") => failed = true,
                _ => {}
            }
            client_sender
                .send(AxumWsMessage::Text(event.data.into()))
                .await?;
        }
    }
    if failed {
        return Ok(None);
    }
    let mut response = completed_response
        .ok_or_else(|| anyhow::anyhow!("custom upstream ended without a terminal response"))?;
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .filter(|response_id| !response_id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("custom completed response is missing id"))?
        .to_owned();
    let output = response
        .get_mut("output")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("custom completed response output must be an array"))?;
    let mut history = match returned_body {
        Some(mut body) => take_custom_request_input(&mut body)?,
        None => preserved_history.expect("fusion request history was preserved"),
    };
    history.append(output);
    Ok(Some(CustomWebSocketState {
        response_id,
        model,
        route,
        history,
    }))
}

async fn expand_custom_websocket_history(
    app_state: &AppState,
    body: &mut Value,
    state: Option<CustomWebSocketState>,
) -> anyhow::Result<()> {
    let Some(previous_response_id) = body
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(());
    };
    let state = state.ok_or_else(|| {
        anyhow::anyhow!("unknown custom previous_response_id: {previous_response_id}")
    })?;
    if previous_response_id != state.response_id {
        anyhow::bail!("unknown custom previous_response_id: {previous_response_id}");
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("custom request is missing model"))?;
    let route = app_state
        .resolve_model_route(model)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if route != state.route {
        anyhow::bail!(
            "custom previous_response_id belongs to model {}",
            state.model
        );
    }
    let incremental_input = body
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("custom incremental input must be an array"))?;
    let mut full_input = state.history;
    full_input.extend(incremental_input.iter().cloned());
    body["input"] = Value::Array(full_input);
    Ok(())
}

async fn complete_custom_noop(
    state: &AppState,
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    mut body: Value,
) -> anyhow::Result<CustomWebSocketState> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("custom noop request is missing model"))?;
    let model = model.to_owned();
    let route = state
        .resolve_model_route(&model)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if route == ResolvedModelRoute::Official {
        anyhow::bail!("official model reached custom websocket noop");
    }
    let history = take_custom_request_input(&mut body)?;
    let response_id = format!("resp_{}", Uuid::new_v4().simple());
    for status in ["in_progress", "completed"] {
        client_sender
            .send(AxumWsMessage::Text(
                json!({
                    "type": if status == "completed" { "response.completed" } else { "response.created" },
                    "response": {
                        "id": response_id,
                        "object": "response",
                        "status": status,
                        "output": []
                    }
                })
                .to_string()
                .into(),
            ))
            .await?;
    }
    tracing::debug!(route = "custom_ws_noop", "completed noop responses request");
    Ok(CustomWebSocketState {
        response_id,
        model,
        route,
        history,
    })
}
