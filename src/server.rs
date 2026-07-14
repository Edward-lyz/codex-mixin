use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower_http::decompression::RequestDecompressionLayer;
use uuid::Uuid;

use crate::anthropic::{BaiduAvailableModelsResponse, ModelsResponse};
use crate::benchmark::{BenchmarkSnapshotResponse, ModelBenchmarkManager, StartBenchmarkRequest};
use crate::catalog::{codex_catalog_from_models_with_metadata, load_template_catalog};
use crate::config::{GatewayConfig, ProviderPreset, UpstreamAuthHeader, UpstreamKind};
use crate::convert::responses_to_anthropic;
use crate::error::GatewayError;
use crate::image_generation::ImageRouteRegistry;
use crate::model_metadata::ModelMetadataResolver;
use crate::openai_chat::responses_to_openai_chat;
use crate::openai_events::{
    map_anthropic_sse_with_image_routes, map_openai_chat_sse_with_image_routes,
};
use crate::sse::drain_events;

type OfficialWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug)]
struct CustomWebSocketState {
    response_id: String,
    model: String,
    history: Vec<Value>,
}

#[derive(Clone)]
pub struct AppState {
    config: Arc<GatewayConfig>,
    client: Client,
    image_routes: ImageRouteRegistry,
    benchmarks: ModelBenchmarkManager,
}

impl AppState {
    pub fn new(config: GatewayConfig) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(64)
            .build()?;
        Ok(Self {
            config: Arc::new(config),
            client,
            image_routes: ImageRouteRegistry::default(),
            benchmarks: ModelBenchmarkManager::from_default_path(),
        })
    }

    fn custom_image_routes(&self) -> Option<ImageRouteRegistry> {
        self.config
            .upstream_image_generation_path
            .is_some()
            .then(|| self.image_routes.clone())
    }

    pub async fn fetch_models(&self) -> Result<Vec<crate::anthropic::ModelInfo>, GatewayError> {
        let response = self
            .apply_upstream_auth(self.client.get(self.config.upstream_models_url()))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(GatewayError::Upstream(format!(
                "models endpoint returned {status}: {body}"
            )));
        }
        let parsed: ModelsResponse = serde_json::from_str(&body)?;
        let mut models = parsed.data;
        if self.config.provider_preset == ProviderPreset::BaiduOneApi {
            let response = self
                .apply_upstream_auth(self.client.post(format!(
                    "{}/openapi/v2/available_models",
                    self.config.upstream_base_url
                )))
                .json(&json!({}))
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                return Err(GatewayError::Upstream(format!(
                    "available models endpoint returned {status}: {body}"
                )));
            }
            let available: BaiduAvailableModelsResponse =
                serde_json::from_str(&body).map_err(|err| {
                    GatewayError::Upstream(format!(
                        "available models endpoint returned invalid JSON: {err}"
                    ))
                })?;
            if !available.success {
                return Err(GatewayError::Upstream(format!(
                    "available models endpoint failed: {}",
                    available.message
                )));
            }
            let mut available_by_model = HashMap::with_capacity(available.data.len());
            for model in available.data {
                if let Some(canonical) = model.model.strip_suffix("-内部") {
                    available_by_model
                        .entry(canonical.to_owned())
                        .or_insert(model);
                } else {
                    available_by_model.insert(model.model.clone(), model);
                }
            }
            models.retain_mut(|model| {
                let Some(available) = available_by_model.get(&model.id) else {
                    return true;
                };
                let Some(capability) = &available.capability else {
                    tracing::warn!(
                        model = %available.model,
                        "excluding available models entry without capability metadata"
                    );
                    return false;
                };
                model.price_type = Some(available.price_type.clone());
                model.description = Some(capability.model_description.clone());
                model.ratio = Some(capability.ratio.clone());
                model.context_window = Some(capability.context_window);
                model.supports_image = Some(capability.supports_image);
                model.supports_thinking = Some(capability.supports_thinking);
                true
            });
        }
        Ok(models)
    }

    fn apply_upstream_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = match self.config.upstream_auth_header {
            UpstreamAuthHeader::AuthorizationBearer => {
                request.bearer_auth(&self.config.upstream_api_key)
            }
            UpstreamAuthHeader::XApiKey => {
                request.header("x-api-key", &self.config.upstream_api_key)
            }
        };
        let request = request.header("anthropic-version", &self.config.anthropic_version);
        if let Some(beta) = &self.config.anthropic_beta {
            request.header("anthropic-beta", beta)
        } else {
            request
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/models", get(models))
        .route("/v1/codex-model-catalog", get(codex_model_catalog))
        .route(
            "/v1/model-benchmarks",
            get(model_benchmarks).post(start_model_benchmarks),
        )
        .route("/v1/responses", get(responses_ws).post(responses))
        .route("/v1/images/generations", post(image_generations))
        .route("/v1/images/edits", post(image_edits))
        .layer(RequestDecompressionLayer::new())
        .with_state(state)
}

pub async fn serve(config: GatewayConfig) -> anyhow::Result<()> {
    let bind = config.bind;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    serve_on_listener(config, listener).await
}

pub async fn serve_on_listener(
    mut config: GatewayConfig,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let bind = listener.local_addr()?;
    config.bind = bind;
    let state = AppState::new(config)?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tracing::info!(%bind, "codex-mixin listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    Json(json!({"ok": true}))
}

async fn models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    let models = state.fetch_models().await?;
    Ok(Json(json!({"object":"list","data":models})).into_response())
}

async fn codex_model_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    let models = state.fetch_models().await?;
    let template = load_template_catalog(None)?;
    let metadata = ModelMetadataResolver::from_default_files()?;
    let catalog = codex_catalog_from_models_with_metadata(
        &models,
        state.config.default_context_window,
        template.as_ref(),
        &metadata,
    );
    Ok(Json(catalog).into_response())
}

async fn model_benchmarks(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    let snapshot = state.benchmarks.snapshot().map_err(GatewayError::Other)?;
    Ok(Json(BenchmarkSnapshotResponse { snapshot }).into_response())
}

async fn start_model_benchmarks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<StartBenchmarkRequest>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    let timeout = std::time::Duration::from_secs(request.timeout_seconds);
    if timeout.is_zero() || timeout > std::time::Duration::from_secs(300) {
        return Err(GatewayError::BadRequest(
            "model benchmark timeout must be between 1 and 300 seconds".to_owned(),
        ));
    }
    let models = tokio::time::timeout(timeout, state.fetch_models())
        .await
        .map_err(|_| GatewayError::Upstream("models endpoint timed out".to_owned()))??;
    let snapshot = state
        .benchmarks
        .start(models, (*state.config).clone(), timeout)
        .map_err(|error| GatewayError::BadRequest(error.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(BenchmarkSnapshotResponse {
            snapshot: Some(snapshot),
        }),
    )
        .into_response())
}

async fn responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    if should_forward_to_official(&body) {
        return forward_official_responses(&state, &headers, body).await;
    }
    normalize_custom_model_alias(&mut body);
    let stream = match state.config.upstream_kind {
        UpstreamKind::AnthropicMessages => {
            let mut converted = responses_to_anthropic(&body, &state.config)?;
            if state.config.provider_preset == ProviderPreset::BaiduOneApi {
                converted.request.metadata = stable_session_id(&headers)?
                    .map(|session_id| json!({"session_id": session_id}));
            }
            let upstream = state
                .apply_upstream_auth(state.client.post(state.config.upstream_messages_url()))
                .header(header::ACCEPT, "text/event-stream")
                .json(&converted.request)
                .send()
                .await?;
            let status = upstream.status();
            if !status.is_success() {
                let body = upstream.text().await.unwrap_or_default();
                return Err(GatewayError::Upstream(format!(
                    "messages endpoint returned {status}: {body}"
                )));
            }
            Body::from_stream(map_anthropic_sse_with_image_routes(
                upstream.bytes_stream(),
                body,
                converted.tool_names,
                state.custom_image_routes(),
            ))
        }
        UpstreamKind::OpenAiChat => {
            let converted = responses_to_openai_chat(&body)?;
            let upstream = state
                .apply_upstream_auth(state.client.post(state.config.upstream_messages_url()))
                .header(header::ACCEPT, "text/event-stream")
                .json(&converted.request)
                .send()
                .await?;
            let status = upstream.status();
            if !status.is_success() {
                let body = upstream.text().await.unwrap_or_default();
                return Err(GatewayError::Upstream(format!(
                    "chat completions endpoint returned {status}: {body}"
                )));
            }
            Body::from_stream(map_openai_chat_sse_with_image_routes(
                upstream.bytes_stream(),
                body,
                converted.tool_names,
                state.custom_image_routes(),
            ))
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(stream)
        .map_err(|err| GatewayError::Other(err.into()))
}

async fn responses_ws(
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
        tracing::warn!(error = %err, "responses websocket failed");
    }
}

async fn route_responses_ws(
    state: AppState,
    headers: HeaderMap,
    client_socket: WebSocket,
) -> anyhow::Result<()> {
    let (mut client_sender, mut client_receiver) = client_socket.split();
    let mut official_socket = None;
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

        if should_forward_to_official(&body) {
            custom_state = None;
            tracing::debug!(
                model,
                route = "official_ws",
                "routing responses websocket request"
            );
            if official_socket.is_none() {
                official_socket = Some(connect_official_responses_ws(&state, &headers).await?);
            }
            proxy_official_responses_ws(
                official_socket
                    .as_mut()
                    .expect("official websocket connected"),
                &mut client_sender,
                &body,
            )
            .await?;
            continue;
        }

        if official_socket.take().is_some() {
            tracing::debug!(
                model,
                "closing official websocket before custom model request"
            );
        }
        tracing::debug!(
            model,
            route = "custom_ws",
            "routing responses websocket request"
        );
        let next_state = match expand_custom_websocket_history(&mut body, custom_state.as_ref()) {
            Ok(()) if is_noop_responses_ws_request(&body) => {
                complete_custom_noop(&mut client_sender, &body)
                    .await
                    .map(Some)
            }
            Ok(()) => proxy_custom_responses_ws(&state, &headers, &mut client_sender, body).await,
            Err(err) => Err(err),
        };
        match next_state {
            Ok(next_state) => custom_state = next_state,
            Err(err) => {
                custom_state = None;
                tracing::warn!(error = %err, "custom responses websocket request failed");
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
        let (authorization, account_id) = read_codex_official_auth(&state.config.codex_auth_path)?;
        request_headers.insert(header::AUTHORIZATION, authorization);
        request_headers.insert("chatgpt-account-id", account_id);
        for name in [
            "openai-beta",
            "x-codex-installation-id",
            "x-codex-beta-features",
            "x-codex-originator",
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
    let (official_socket, _) = connect_async(request).await?;
    Ok(official_socket)
}

async fn proxy_official_responses_ws(
    official_socket: &mut OfficialWebSocket,
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    body: &Value,
) -> anyhow::Result<()> {
    official_socket
        .send(TungsteniteMessage::Text(body.to_string().into()))
        .await?;
    while let Some(message) = official_socket.next().await {
        let message = message?;
        let terminal = match &message {
            TungsteniteMessage::Text(text) => serde_json::from_str::<Value>(text).ok(),
            TungsteniteMessage::Binary(bytes) => serde_json::from_slice::<Value>(bytes).ok(),
            _ => None,
        }
        .and_then(|event| event.get("type").and_then(Value::as_str).map(str::to_owned))
        .is_some_and(|event_type| {
            matches!(
                event_type.as_str(),
                "response.completed" | "response.failed" | "response.incomplete"
            )
        });
        match message {
            TungsteniteMessage::Ping(bytes) => {
                official_socket
                    .send(TungsteniteMessage::Pong(bytes))
                    .await?;
            }
            TungsteniteMessage::Pong(_) | TungsteniteMessage::Frame(_) => {}
            TungsteniteMessage::Close(_) => {
                anyhow::bail!("official responses websocket closed before a terminal response")
            }
            message => {
                if let Some(message) = tungstenite_to_axum_message(message) {
                    client_sender.send(message).await?;
                }
            }
        }
        if terminal {
            return Ok(());
        }
    }
    anyhow::bail!("official responses websocket ended before a terminal response")
}

async fn proxy_custom_responses_ws(
    state: &AppState,
    headers: &HeaderMap,
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    mut body: Value,
) -> anyhow::Result<Option<CustomWebSocketState>> {
    normalize_custom_model_alias(&mut body);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("custom request is missing model"))?
        .to_owned();
    let mut history = body
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("custom request input must be an array"))?
        .clone();
    let stream: futures_util::stream::BoxStream<
        'static,
        Result<bytes::Bytes, std::convert::Infallible>,
    > = match state.config.upstream_kind {
        UpstreamKind::AnthropicMessages => {
            let mut converted = responses_to_anthropic(&body, &state.config)?;
            if state.config.provider_preset == ProviderPreset::BaiduOneApi {
                converted.request.metadata =
                    stable_session_id(headers)?.map(|session_id| json!({"session_id": session_id}));
            }
            let upstream = state
                .apply_upstream_auth(state.client.post(state.config.upstream_messages_url()))
                .header(header::ACCEPT, "text/event-stream")
                .json(&converted.request)
                .send()
                .await?;
            let status = upstream.status();
            if !status.is_success() {
                let body = upstream.text().await?;
                anyhow::bail!("messages endpoint returned {status}: {body}");
            }
            map_anthropic_sse_with_image_routes(
                upstream.bytes_stream(),
                body,
                converted.tool_names,
                state.custom_image_routes(),
            )
            .boxed()
        }
        UpstreamKind::OpenAiChat => {
            let converted = responses_to_openai_chat(&body)?;
            let upstream = state
                .apply_upstream_auth(state.client.post(state.config.upstream_messages_url()))
                .header(header::ACCEPT, "text/event-stream")
                .json(&converted.request)
                .send()
                .await?;
            let status = upstream.status();
            if !status.is_success() {
                let body = upstream.text().await?;
                anyhow::bail!("chat completions endpoint returned {status}: {body}");
            }
            map_openai_chat_sse_with_image_routes(
                upstream.bytes_stream(),
                body,
                converted.tool_names,
                state.custom_image_routes(),
            )
            .boxed()
        }
    };
    tokio::pin!(stream);
    let mut buffer = Vec::new();
    let mut completed_response = None;
    let mut failed = false;
    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(never) => match never {},
        };
        buffer.extend_from_slice(&bytes);
        for event in drain_events(&mut buffer) {
            let payload: Value = serde_json::from_str(&event.data)?;
            match payload.get("type").and_then(Value::as_str) {
                Some("response.completed") => {
                    completed_response = payload.get("response").cloned();
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
    let response = completed_response
        .ok_or_else(|| anyhow::anyhow!("custom upstream ended without a terminal response"))?;
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .filter(|response_id| !response_id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("custom completed response is missing id"))?
        .to_owned();
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("custom completed response output must be an array"))?;
    history.extend(output.iter().cloned());
    Ok(Some(CustomWebSocketState {
        response_id,
        model,
        history,
    }))
}

fn expand_custom_websocket_history(
    body: &mut Value,
    state: Option<&CustomWebSocketState>,
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
    if model.strip_suffix("-custom").unwrap_or(model) != state.model {
        anyhow::bail!(
            "custom previous_response_id belongs to model {}",
            state.model
        );
    }
    let incremental_input = body
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("custom incremental input must be an array"))?;
    let mut full_input = state.history.clone();
    full_input.extend(incremental_input.iter().cloned());
    body["input"] = Value::Array(full_input);
    Ok(())
}

async fn complete_custom_noop(
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    body: &Value,
) -> anyhow::Result<CustomWebSocketState> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("custom noop request is missing model"))?;
    let model = model.strip_suffix("-custom").unwrap_or(model).to_owned();
    let history = body
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("custom noop input must be an array"))?
        .clone();
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

async fn forward_official_responses(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, GatewayError> {
    let (authorization, account_id) =
        read_codex_official_auth(&state.config.codex_auth_path).map_err(GatewayError::Other)?;
    let upstream = forward_official_headers(
        state
            .client
            .post(&state.config.official_responses_url)
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "text/event-stream"),
        headers,
    )
    .json(&body)
    .send()
    .await?;
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_owned();
    if !status.is_success() {
        let body = upstream.text().await.unwrap_or_default();
        return Err(GatewayError::Upstream(format!(
            "official responses endpoint returned {status}: {body}"
        )));
    }
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|err| GatewayError::Other(err.into()))
}

async fn image_generations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    let routed_prompt = body
        .get("prompt")
        .and_then(Value::as_str)
        .map(|prompt| state.image_routes.resolve_prompt(prompt))
        .transpose()
        .map_err(GatewayError::BadRequest)?
        .flatten();
    if let Some(prompt) = routed_prompt {
        body["prompt"] = Value::String(prompt);
        let url = state
            .config
            .upstream_image_generation_url()
            .ok_or_else(|| {
                GatewayError::Other(anyhow::anyhow!(
                    "routed image request has no configured upstream image generation endpoint"
                ))
            })?;
        let request = state
            .client
            .post(url)
            .header(header::ACCEPT, "application/json");
        let request = match state.config.upstream_auth_header {
            UpstreamAuthHeader::AuthorizationBearer => {
                request.bearer_auth(&state.config.upstream_api_key)
            }
            UpstreamAuthHeader::XApiKey => {
                request.header("x-api-key", &state.config.upstream_api_key)
            }
        };
        let upstream = request.json(&body).send().await?;
        return proxy_image_response(upstream, "upstream").await;
    }
    let url = state
        .config
        .official_image_generation_url()
        .map_err(GatewayError::Other)?;
    forward_official_image_request(&state, &headers, &body, url).await
}

async fn image_edits(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    if body
        .get("prompt")
        .and_then(Value::as_str)
        .map(|prompt| state.image_routes.resolve_prompt(prompt))
        .transpose()
        .map_err(GatewayError::BadRequest)?
        .flatten()
        .is_some()
    {
        return Err(GatewayError::BadRequest(
            "custom upstream image editing is not supported".to_owned(),
        ));
    }
    let url = state
        .config
        .official_image_edit_url()
        .map_err(GatewayError::Other)?;
    forward_official_image_request(&state, &headers, &body, url).await
}

async fn forward_official_image_request(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
    url: String,
) -> Result<Response, GatewayError> {
    let (authorization, account_id) =
        read_codex_official_auth(&state.config.codex_auth_path).map_err(GatewayError::Other)?;
    let request = forward_official_headers(
        state
            .client
            .post(url)
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "application/json"),
        headers,
    );
    let upstream = request.json(body).send().await?;
    proxy_image_response(upstream, "official").await
}

async fn proxy_image_response(
    upstream: reqwest::Response,
    endpoint: &str,
) -> Result<Response, GatewayError> {
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_owned();
    if !status.is_success() {
        let body = upstream.text().await?;
        return Err(GatewayError::Upstream(format!(
            "{endpoint} image endpoint returned {status}: {body}"
        )));
    }
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|err| GatewayError::Other(err.into()))
}

fn read_codex_official_auth(
    auth_path: &std::path::Path,
) -> anyhow::Result<(axum::http::HeaderValue, axum::http::HeaderValue)> {
    let raw = std::fs::read_to_string(auth_path)
        .map_err(|err| anyhow::anyhow!("read Codex auth file {}: {err}", auth_path.display()))?;
    let auth: Value = serde_json::from_str(&raw)
        .map_err(|err| anyhow::anyhow!("parse Codex auth file {}: {err}", auth_path.display()))?;
    let tokens = auth
        .get("tokens")
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain tokens"))?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain access_token"))?;
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|account_id| !account_id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain account_id"))?;
    Ok((
        format!("Bearer {access_token}").parse()?,
        account_id.parse()?,
    ))
}

fn forward_official_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for name in [
        "openai-beta",
        "x-codex-installation-id",
        "x-codex-beta-features",
        "x-codex-originator",
        "x-codex-turn-state",
        "x-codex-turn-metadata",
        "x-codex-parent-thread-id",
        "x-oai-attestation",
        "x-responsesapi-include-timing-metrics",
        "x-openai-internal-codex-responses-lite",
        "openai-organization",
        "openai-project",
        "user-agent",
        "accept-language",
        "session-id",
        "thread-id",
        "x-client-request-id",
        "x-codex-window-id",
    ] {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
        }
    }
    request
}

fn should_forward_to_official(body: &Value) -> bool {
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return false;
    };
    is_gpt_model(model) && !model.ends_with("-custom")
}

fn normalize_custom_model_alias(body: &mut Value) {
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return;
    };
    if let Some(canonical) = model.strip_suffix("-custom") {
        body["model"] = Value::String(canonical.to_owned());
    }
}

fn is_gpt_model(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("gpt-")
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn check_gateway_auth(state: &AppState, headers: &HeaderMap) -> Result<(), GatewayError> {
    use subtle::ConstantTimeEq;

    let Some(expected) = &state.config.gateway_api_key else {
        return Ok(());
    };
    let actual = bearer_token(headers);
    let accepts_codex_oauth =
        state.config.accept_codex_oauth && state.config.bind.ip().is_loopback() && actual.is_some();
    let gateway_key_matches =
        actual.is_some_and(|actual| actual.as_bytes().ct_eq(expected.as_bytes()).into());
    if accepts_codex_oauth || gateway_key_matches {
        Ok(())
    } else {
        Err(GatewayError::Unauthorized)
    }
}

fn stable_session_id(headers: &HeaderMap) -> Result<Option<&str>, GatewayError> {
    headers
        .get("session-id")
        .map(|value| value.to_str())
        .transpose()
        .map_err(|err| GatewayError::BadRequest(format!("invalid session-id header: {err}")))
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use bytes::Bytes;

    use super::*;
    use crate::benchmark::ModelBenchmarkManager;
    use crate::config::{ThinkingMode, UpstreamAuthHeader};

    #[tokio::test]
    async fn benchmark_api_runs_after_the_start_request_returns_and_persists_results() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);
        let upstream = Router::new()
            .route(
                "/v1/models",
                get(|| async {
                    Json(json!({
                        "object":"list",
                        "data":[{"id":"benchmark-model","object":"model"}]
                    }))
                }),
            )
            .route(
                "/v1/messages",
                post(move |Json(body): Json<Value>| {
                    let captured_requests = Arc::clone(&captured_requests);
                    async move {
                        captured_requests.lock().unwrap().push(body);
                        let stream = async_stream::stream! {
                            yield Ok::<_, Infallible>(Bytes::from(concat!(
                                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n",
                                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"
                            )));
                            tokio::time::sleep(Duration::from_millis(15)).await;
                            yield Ok::<_, Infallible>(Bytes::from(
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"x\"}}\n\n"
                            ));
                            tokio::time::sleep(Duration::from_millis(15)).await;
                            yield Ok::<_, Infallible>(Bytes::from(
                                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"y\"}}\n\n"
                            ));
                            tokio::time::sleep(Duration::from_millis(15)).await;
                            yield Ok::<_, Infallible>(Bytes::from(concat!(
                                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":100}}\n\n",
                                "data: {\"type\":\"message_stop\"}\n\n"
                            )));
                        };
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(header::CONTENT_TYPE, "text/event-stream")
                            .body(Body::from_stream(stream))
                            .unwrap()
                    }
                }),
            );
        let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = upstream_listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(upstream_listener, upstream).await.unwrap();
        });

        let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway_address = gateway_listener.local_addr().unwrap();
        let results_directory = tempfile::tempdir().unwrap();
        let results_path = results_directory.path().join("model-benchmarks.json");
        let mut state = AppState::new(GatewayConfig {
            bind: gateway_address,
            provider_preset: ProviderPreset::Custom,
            upstream_kind: UpstreamKind::AnthropicMessages,
            upstream_base_url: format!("http://{upstream_address}"),
            upstream_messages_path: "/v1/messages".to_owned(),
            upstream_models_path: "/v1/models".to_owned(),
            upstream_image_generation_path: None,
            upstream_api_key: "upstream-key".to_owned(),
            quota_url: None,
            quota_username: None,
            official_responses_url: "https://example.invalid/responses".to_owned(),
            codex_auth_path: results_directory.path().join("auth.json"),
            upstream_auth_header: UpstreamAuthHeader::AuthorizationBearer,
            anthropic_version: "2023-06-01".to_owned(),
            anthropic_beta: None,
            gateway_api_key: Some("gateway-key".to_owned()),
            accept_codex_oauth: false,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(2),
            thinking_mode: ThinkingMode::Off,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            web_search_exclusive: true,
            web_search_omit_system_instructions: true,
            web_search_latest_user_only: true,
        })
        .unwrap();
        state.benchmarks = ModelBenchmarkManager::new(results_path.clone());
        tokio::spawn(async move {
            axum::serve(gateway_listener, router(state)).await.unwrap();
        });

        let client = Client::new();
        let started: Value = client
            .post(format!("http://{gateway_address}/v1/model-benchmarks"))
            .bearer_auth("gateway-key")
            .json(&json!({"timeout_seconds":1}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(started["snapshot"]["status"], "running");

        for _ in 0..100 {
            let response: Value = client
                .get(format!("http://{gateway_address}/v1/model-benchmarks"))
                .bearer_auth("gateway-key")
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            if response["snapshot"]["status"] == "completed" {
                assert_eq!(response["snapshot"]["results"][0]["output_tokens"], 100);
                assert!(response["snapshot"]["results"][0]["tps"].is_number());
                assert!(results_path.exists());
                let request = &requests.lock().unwrap()[0];
                assert_eq!(request["max_tokens"], 100);
                assert_eq!(
                    request["messages"][0]["content"][0]["text"],
                    crate::benchmark::BENCHMARK_PROMPT
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("benchmark API did not finish");
    }
}
