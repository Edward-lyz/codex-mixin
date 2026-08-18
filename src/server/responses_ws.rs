use super::auth::{FORWARDED_OFFICIAL_HEADERS, check_gateway_auth, stable_oneapi_routing};
use super::websocket_proxy::connect_upstream_websocket;
use super::*;
use memchr::memmem;

type OfficialWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type ResponsesClientSender = SplitSink<WebSocket, AxumWsMessage>;

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
    let (mut body, _) = crate::gateway::normalize_provider_images_blocking(body).await?;
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

async fn run_custom_ws_request(
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

async fn proxy_official_ws_request(
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

fn effective_official_cache_body(body: &Value, request_history: Option<&[Value]>) -> Value {
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

fn official_websocket_request_history(
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

fn take_custom_request_input(body: &mut Value) -> anyhow::Result<Vec<Value>> {
    match body
        .as_object_mut()
        .and_then(|request| request.remove("input"))
    {
        Some(Value::Array(input)) => Ok(input),
        _ => anyhow::bail!("custom request input must be an array"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_cache_shape_expands_history_without_rewriting_wire_request() {
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
