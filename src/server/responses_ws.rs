use super::auth::check_gateway_auth;
use super::*;

mod custom;
mod official;
use custom::{CustomWebSocketState, run_custom_ws_request};
use official::{
    OfficialWebSocketState, effective_official_cache_body, official_websocket_request_history,
    proxy_official_ws_request,
};

type OfficialWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type ResponsesClientSender = SplitSink<WebSocket, AxumWsMessage>;

struct ResponsesWsContext<'a> {
    state: &'a AppState,
    headers: &'a HeaderMap,
    client_sender: &'a mut ResponsesClientSender,
    official_socket: &'a mut Option<OfficialWebSocket>,
    official_state: &'a mut Option<OfficialWebSocketState>,
    custom_state: &'a mut Option<CustomWebSocketState>,
}

pub(super) async fn responses_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    Ok(ws
        .max_message_size(crate::request_body::MAX_REQUEST_BYTES)
        .on_upgrade(move |socket| handle_responses_ws(state, headers, socket))
        .into_response())
}

async fn handle_responses_ws(state: AppState, headers: HeaderMap, client_socket: WebSocket) {
    if let Err(err) = route_responses_ws(state, headers, client_socket).await {
        tracing::warn!(
            error = %format!("{err:#}"),
            "responses websocket failed"
        );
    }
}

async fn route_responses_ws(
    state: AppState,
    headers: HeaderMap,
    client_socket: WebSocket,
) -> anyhow::Result<()> {
    let (mut client_sender, mut client_receiver) = client_socket.split();
    let mut official_socket = None;
    let mut official_state = None;
    let mut custom_state = None;

    loop {
        let Some(mut body) =
            next_responses_ws_body(&mut client_sender, &mut client_receiver).await?
        else {
            return Ok(());
        };
        if body.get("stream").is_none() {
            body["stream"] = Value::Bool(true);
        }
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("<missing>")
            .to_owned();

        let mut context = ResponsesWsContext {
            state: &state,
            headers: &headers,
            client_sender: &mut client_sender,
            official_socket: &mut official_socket,
            official_state: &mut official_state,
            custom_state: &mut custom_state,
        };
        if matches!(
            state.resolve_model_route(&model).await,
            Ok(ResolvedModelRoute::Official)
        ) {
            handle_official_ws_request(&mut context, body, &model).await?;
            continue;
        }

        handle_custom_ws_request(&mut context, body, &model).await?;
    }
}

async fn handle_official_ws_request(
    context: &mut ResponsesWsContext<'_>,
    body: Value,
    model: &str,
) -> anyhow::Result<()> {
    let body = super::responses_http::normalize_official_responses_body(body);
    let (mut body, _) = crate::images::normalize_provider_images_blocking(body).await?;
    *context.custom_state = None;
    tracing::debug!(
        model,
        route = "official_ws",
        "routing responses websocket request"
    );
    let mut request_history =
        match official_websocket_request_history(&body, context.official_state.take()) {
            Ok(history) => history,
            Err(error) => {
                *context.official_socket = None;
                *context.official_state = None;
                send_responses_ws_failure(
                    context.client_sender,
                    None,
                    error,
                    "invalid_request_error",
                )
                .await?;
                return Ok(());
            }
        };
    let effective_body = effective_official_cache_body(&body, request_history.as_deref());
    let observation = super::responses_http::official_prefix_observation(
        context.state,
        context.headers,
        &effective_body,
    )?;
    let mut usage_observer = observation.map(crate::gateway::UpstreamCacheObserver::new);
    let request_error = proxy_official_ws_request(
        context,
        &mut body,
        model,
        &mut request_history,
        &mut usage_observer,
    )
    .await?;
    if let Some((error, response_id)) = request_error {
        *context.official_state = None;
        tracing::warn!(model, error = %error, "official responses websocket request failed");
        send_responses_ws_failure(context.client_sender, response_id, error, "server_error")
            .await?;
    }
    Ok(())
}

async fn handle_custom_ws_request(
    context: &mut ResponsesWsContext<'_>,
    mut body: Value,
    model: &str,
) -> anyhow::Result<()> {
    disconnect_official_ws_for_custom_request(context, model);
    tracing::debug!(
        model,
        route = "custom_ws",
        "routing responses websocket request"
    );
    let next_state = run_custom_ws_request(context, &mut body).await;
    match next_state {
        Ok(state) => *context.custom_state = state,
        Err(error) => {
            *context.custom_state = None;
            tracing::warn!(
                model,
                error = %format!("{error:#}"),
                "custom responses websocket request failed"
            );
            send_responses_ws_failure(context.client_sender, None, error, "invalid_request_error")
                .await?;
        }
    }
    Ok(())
}

fn disconnect_official_ws_for_custom_request(context: &mut ResponsesWsContext<'_>, model: &str) {
    if context.official_socket.take().is_some() {
        tracing::debug!(
            model,
            "closing official websocket before custom model request"
        );
    }
    *context.official_state = None;
}

async fn send_responses_ws_failure(
    client_sender: &mut ResponsesClientSender,
    response_id: Option<String>,
    error: impl std::fmt::Display,
    code: &str,
) -> anyhow::Result<()> {
    client_sender
        .send(AxumWsMessage::Text(
            crate::sse::response_failed_payload(response_id, None, error.to_string(), code)
                .to_string()
                .into(),
        ))
        .await?;
    Ok(())
}

fn take_custom_request_input(body: &mut Value) -> anyhow::Result<Vec<Value>> {
    match body
        .as_object_mut()
        .and_then(|request| request.remove("input"))
    {
        Some(Value::Array(input)) => Ok(input),
        _ => anyhow::bail!("custom request input must be an array"),
    }
}

async fn next_responses_ws_body(
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    client_receiver: &mut SplitStream<WebSocket>,
) -> anyhow::Result<Option<Value>> {
    loop {
        match client_receiver.next().await {
            Some(Ok(message @ (AxumWsMessage::Text(_) | AxumWsMessage::Binary(_)))) => {
                return Ok(Some(responses_ws_body(&message)?));
            }
            Some(Ok(AxumWsMessage::Ping(bytes))) => {
                client_sender.send(AxumWsMessage::Pong(bytes)).await?;
            }
            Some(Ok(AxumWsMessage::Pong(_))) => {}
            Some(Ok(AxumWsMessage::Close(_))) | None => return Ok(None),
            Some(Err(err)) => return Err(err.into()),
        }
    }
}

fn responses_ws_body(message: &AxumWsMessage) -> anyhow::Result<Value> {
    match message {
        AxumWsMessage::Text(text) => Ok(serde_json::from_str(text)?),
        AxumWsMessage::Binary(bytes) => Ok(serde_json::from_slice(bytes)?),
        other => {
            anyhow::bail!("responses websocket frame must be JSON text or binary, got {other:?}")
        }
    }
}

fn tungstenite_to_axum_message(message: TungsteniteMessage) -> Option<AxumWsMessage> {
    match message {
        TungsteniteMessage::Text(text) => Some(AxumWsMessage::Text(text.to_string().into())),
        TungsteniteMessage::Binary(bytes) => Some(AxumWsMessage::Binary(bytes)),
        TungsteniteMessage::Ping(bytes) => Some(AxumWsMessage::Ping(bytes)),
        TungsteniteMessage::Pong(bytes) => Some(AxumWsMessage::Pong(bytes)),
        TungsteniteMessage::Close(_) => Some(AxumWsMessage::Close(None)),
        TungsteniteMessage::Frame(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_cache_shape_expands_history_without_rewriting_wire_request() {
        use super::official::effective_official_cache_body;

        let body = json!({
            "type": "response.create",
            "model": "gpt-5.6-sol",
            "previous_response_id": "resp_1",
            "input": [{"type":"message","role":"user","content":"next"}]
        });
        let history = vec![
            json!({"type":"message","role":"user","content":"first"}),
            json!({"type":"message","role":"assistant","content":"answer"}),
            json!({"type":"message","role":"user","content":"next"}),
        ];

        let effective = effective_official_cache_body(&body, Some(&history));

        assert_eq!(body["previous_response_id"], "resp_1");
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
        assert!(effective.get("previous_response_id").is_none());
        assert_eq!(effective["input"].as_array().unwrap(), &history);
    }
}
