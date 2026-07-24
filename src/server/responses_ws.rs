use super::auth::{check_gateway_auth, stable_oneapi_routing};
use super::*;

type OfficialWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug)]
struct OfficialWebSocketRequestError {
    source: anyhow::Error,
    response_started: bool,
    response_id: Option<String>,
}

#[derive(Debug)]
struct OfficialWebSocketState {
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

#[derive(Debug)]
struct CustomWebSocketState {
    response_id: String,
    model: String,
    route: ResolvedModelRoute,
    history: Vec<Value>,
}

pub(super) async fn responses_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    Ok(ws
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

        if matches!(
            state.resolve_model_route(&model),
            Ok(ResolvedModelRoute::Official)
        ) {
            custom_state = None;
            tracing::debug!(
                model = model.as_str(),
                route = "official_ws",
                "routing responses websocket request"
            );
            let request_history =
                match official_websocket_request_history(&body, official_state.take()) {
                    Ok(history) => history,
                    Err(err) => {
                        official_socket = None;
                        official_state = None;
                        let message = err.to_string();
                        let error = json!({"message": message, "type": "invalid_request_error"});
                        client_sender
                            .send(AxumWsMessage::Text(
                                json!({
                                    "type": "response.failed",
                                    "response": {
                                        "id": format!("resp_{}", Uuid::new_v4().simple()),
                                        "object": "response",
                                        "status": "failed",
                                        "error": error,
                                        "output": []
                                    },
                                    "error": error
                                })
                                .to_string()
                                .into(),
                            ))
                            .await?;
                        continue;
                    }
                };
            let mut retry_available = true;
            let request_error = loop {
                if official_socket.is_none() {
                    match connect_official_responses_ws(&state, &headers).await {
                        Ok(socket) => {
                            official_socket = Some(socket);
                            if body.get("previous_response_id").is_some() {
                                body["input"] = Value::Array(request_history.clone());
                                body.as_object_mut()
                                    .expect("responses request is an object")
                                    .remove("previous_response_id");
                            }
                        }
                        Err(err) if retry_available => {
                            retry_available = false;
                            tracing::warn!(
                                model = model.as_str(),
                                error = %err,
                                "retrying official responses websocket connection"
                            );
                            continue;
                        }
                        Err(err) => break Some((err, None)),
                    }
                }
                match proxy_official_responses_ws(
                    official_socket
                        .as_mut()
                        .expect("official websocket connected"),
                    &mut client_sender,
                    &body,
                    state.config.request_timeout,
                )
                .await
                {
                    Ok(OfficialWebSocketResponse::Completed {
                        response_id,
                        items_added,
                    }) => {
                        let mut history = request_history;
                        history.extend(items_added);
                        official_state = Some(OfficialWebSocketState {
                            response_id,
                            model: model.clone(),
                            history,
                        });
                        break None;
                    }
                    Ok(OfficialWebSocketResponse::Failed) => {
                        official_socket = None;
                        official_state = None;
                        break None;
                    }
                    Err(err) if !err.response_started && retry_available => {
                        retry_available = false;
                        official_socket = None;
                        tracing::warn!(
                            model = model.as_str(),
                            error = %err.source,
                            "reconnecting stale official responses websocket"
                        );
                    }
                    Err(err) => {
                        official_socket = None;
                        break Some((err.source, err.response_id));
                    }
                }
            };
            if let Some((err, response_id)) = request_error {
                official_state = None;
                tracing::warn!(
                    model = model.as_str(),
                    error = %err,
                    "official responses websocket request failed"
                );
                let message = err.to_string();
                let error = json!({"message": message, "type": "server_error"});
                let response_id =
                    response_id.unwrap_or_else(|| format!("resp_{}", Uuid::new_v4().simple()));
                client_sender
                    .send(AxumWsMessage::Text(
                        json!({
                            "type": "response.failed",
                            "response": {
                                "id": response_id,
                                "object": "response",
                                "status": "failed",
                                "error": error,
                                "output": []
                            },
                            "error": error
                        })
                        .to_string()
                        .into(),
                    ))
                    .await?;
            }
            continue;
        }

        if official_socket.take().is_some() {
            tracing::debug!(
                model,
                "closing official websocket before custom model request"
            );
        }
        official_state = None;
        tracing::debug!(
            model,
            route = "custom_ws",
            "routing responses websocket request"
        );
        let next_state =
            match expand_custom_websocket_history(&state, &mut body, custom_state.take()) {
                Ok(()) if is_noop_responses_ws_request(&body) => {
                    complete_custom_noop(&state, &mut client_sender, body)
                        .await
                        .map(Some)
                }
                Ok(()) => {
                    proxy_custom_responses_ws(&state, &headers, &mut client_sender, body).await
                }
                Err(err) => Err(err),
            };
        match next_state {
            Ok(next_state) => custom_state = next_state,
            Err(err) => {
                custom_state = None;
                tracing::warn!(
                    model = model.as_str(),
                    error = %format!("{err:#}"),
                    "custom responses websocket request failed"
                );
                let message = err.to_string();
                let error = json!({"message": message, "type": "invalid_request_error"});
                client_sender
                    .send(AxumWsMessage::Text(
                        json!({
                            "type": "response.failed",
                            "response": {
                                "id": format!("resp_{}", Uuid::new_v4().simple()),
                                "object": "response",
                                "status": "failed",
                                "error": error,
                                "output": []
                            },
                            "error": error
                        })
                        .to_string()
                        .into(),
                    ))
                    .await?;
            }
        }
    }
}

fn is_noop_responses_ws_request(body: &Value) -> bool {
    if body.get("generate").and_then(Value::as_bool) == Some(false) {
        return true;
    }
    body.get("input")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
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
        for name in [
            "openai-beta",
            "x-codex-installation-id",
            "x-codex-beta-features",
            "originator",
            "x-codex-originator",
            "x-openai-subagent",
            "x-openai-memgen-request",
            "x-codex-turn-state",
            "x-codex-turn-metadata",
            "x-codex-parent-thread-id",
            "x-oai-attestation",
            "x-responsesapi-include-timing-metrics",
            "accept-language",
            "user-agent",
            "session-id",
            "thread-id",
            "x-client-request-id",
            "x-codex-window-id",
        ] {
            if let Some(value) = headers.get(name) {
                request_headers.insert(name, value.clone());
            }
        }
    }
    let (official_socket, _) =
        tokio::time::timeout(state.config.request_timeout, connect_async(request))
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
            TungsteniteMessage::Text(text) => serde_json::from_str::<Value>(text).ok(),
            TungsteniteMessage::Binary(bytes) => serde_json::from_slice::<Value>(bytes).ok(),
            _ => None,
        };
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

fn official_websocket_request_history(
    body: &Value,
    state: Option<OfficialWebSocketState>,
) -> anyhow::Result<Vec<Value>> {
    let incremental_input = body
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("official request input must be an array"))?;
    let Some(previous_response_id) = body.get("previous_response_id").and_then(Value::as_str)
    else {
        return Ok(incremental_input.clone());
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
    Ok(history)
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
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let model = requested_model;
    if !body.get("input").is_some_and(Value::is_array) {
        anyhow::bail!("custom request input must be an array");
    }
    let provider_routing = stable_oneapi_routing(headers, &body)?;
    let mut history = body
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("custom request input must be an array"))?;
    let stream = match &route {
        ResolvedModelRoute::Official => {
            anyhow::bail!("official model reached custom websocket proxy")
        }
        ResolvedModelRoute::Provider { .. } => {
            stream_response_with_options(state, body, provider_routing.as_ref(), None).await?
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
                FusionEngine::new(state, &profile)
                    .with_headers(headers.clone())
                    .stream_with_routing(body, provider_routing)
            } else {
                body["stream"] = Value::Bool(true);
                FusionEngine::new(state, &profile)
                    .with_headers(headers.clone())
                    .stream_final_continuation(body, provider_routing.as_ref())
                    .await?
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
    history.append(output);
    Ok(Some(CustomWebSocketState {
        response_id,
        model,
        route,
        history,
    }))
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

fn expand_custom_websocket_history(
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

fn websocket_url_from_http_url(url: &str) -> anyhow::Result<String> {
    if let Some(rest) = url.strip_prefix("https://") {
        return Ok(format!("wss://{rest}"));
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return Ok(format!("ws://{rest}"));
    }
    anyhow::bail!("official responses URL must start with http:// or https://")
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
