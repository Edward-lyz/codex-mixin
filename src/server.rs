use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::anthropic::ModelsResponse;
use crate::catalog::{codex_catalog_from_models_with_metadata, load_template_catalog};
use crate::codex_auth::refresh_codex_official_auth;
use crate::config::{GatewayConfig, UpstreamAuthHeader, UpstreamKind};
use crate::convert::responses_to_anthropic;
use crate::error::GatewayError;
use crate::model_metadata::ModelMetadataResolver;
use crate::openai_chat::responses_to_openai_chat;
use crate::openai_events::{map_anthropic_sse, map_openai_chat_sse};
use crate::sse::drain_events;

#[derive(Clone)]
pub struct AppState {
    config: Arc<GatewayConfig>,
    client: Client,
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
        })
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
        Ok(parsed.data)
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
        .route("/v1/responses", get(responses_ws).post(responses))
        .with_state(state)
}

pub async fn serve(config: GatewayConfig) -> anyhow::Result<()> {
    let bind = config.bind;
    let state = AppState::new(config)?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "codex-mixin listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
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
            let converted = responses_to_anthropic(&body, &state.config)?;
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
            Body::from_stream(map_anthropic_sse(
                upstream.bytes_stream(),
                body,
                converted.tool_names,
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
            Body::from_stream(map_openai_chat_sse(
                upstream.bytes_stream(),
                body,
                converted.tool_names,
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
    let first_message = match client_receiver.next().await {
        Some(message) => message?,
        None => return Ok(()),
    };
    let mut body = responses_ws_body(&first_message)?;
    if body.get("stream").is_none() {
        body["stream"] = Value::Bool(true);
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("<missing>");
    if should_forward_to_official(&body) {
        tracing::debug!(model, route = "official_ws", "routing responses websocket");
        tunnel_official_responses_ws(
            state,
            headers,
            client_sender,
            client_receiver,
            first_message,
        )
        .await
    } else {
        tracing::debug!(model, route = "custom_ws", "routing responses websocket");
        route_custom_responses_ws(&state, &mut client_sender, &mut client_receiver, body).await
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

async fn tunnel_official_responses_ws(
    state: AppState,
    headers: HeaderMap,
    mut client_sender: SplitSink<WebSocket, AxumWsMessage>,
    mut client_receiver: SplitStream<WebSocket>,
    first_message: AxumWsMessage,
) -> anyhow::Result<()> {
    let official_auth = refresh_codex_official_auth(
        &state.client,
        &state.config.codex_auth_path,
        &state.config.official_oauth_token_url,
    )
    .await?;
    let websocket_url = websocket_url_from_http_url(&state.config.official_responses_url)?;
    let mut request = websocket_url.into_client_request()?;
    {
        let request_headers = request.headers_mut();
        request_headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", official_auth.access_token).parse()?,
        );
        request_headers.insert("chatgpt-account-id", official_auth.account_id.parse()?);
        for name in [
            "x-codex-installation-id",
            "accept-language",
            "user-agent",
            "session_id",
            "x-client-request-id",
            "x-codex-window-id",
        ] {
            if let Some(value) = headers.get(name) {
                request_headers.insert(name, value.clone());
            }
        }
    }
    let (official_socket, _) = connect_async(request).await?;
    let (mut official_sender, mut official_receiver) = official_socket.split();
    if let Some(message) = axum_to_tungstenite_message(first_message) {
        official_sender.send(message).await?;
    }

    let client_to_official = async {
        while let Some(message) = client_receiver.next().await {
            let message = message?;
            let Some(message) = axum_to_tungstenite_message(message) else {
                break;
            };
            official_sender.send(message).await?;
        }
        anyhow::Ok(())
    };
    let official_to_client = async {
        while let Some(message) = official_receiver.next().await {
            let message = message?;
            let Some(message) = tungstenite_to_axum_message(message) else {
                break;
            };
            client_sender.send(message).await?;
        }
        anyhow::Ok(())
    };
    tokio::select! {
        result = client_to_official => result,
        result = official_to_client => result,
    }
}

async fn proxy_custom_responses_ws(
    state: &AppState,
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    mut body: Value,
) -> anyhow::Result<()> {
    normalize_custom_model_alias(&mut body);
    let stream: futures_util::stream::BoxStream<
        'static,
        Result<bytes::Bytes, std::convert::Infallible>,
    > = match state.config.upstream_kind {
        UpstreamKind::AnthropicMessages => {
            let converted = responses_to_anthropic(&body, &state.config)?;
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
            map_anthropic_sse(upstream.bytes_stream(), body, converted.tool_names).boxed()
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
            map_openai_chat_sse(upstream.bytes_stream(), body, converted.tool_names).boxed()
        }
    };
    tokio::pin!(stream);
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(never) => match never {},
        };
        buffer.extend_from_slice(&bytes);
        for event in drain_events(&mut buffer) {
            client_sender
                .send(AxumWsMessage::Text(event.data.into()))
                .await?;
        }
    }
    Ok(())
}

async fn route_custom_responses_ws(
    state: &AppState,
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    client_receiver: &mut SplitStream<WebSocket>,
    mut body: Value,
) -> anyhow::Result<()> {
    loop {
        if is_noop_responses_ws_request(&body) {
            tracing::debug!(route = "custom_ws_noop", "ignoring noop responses request");
        } else {
            if should_forward_to_official(&body) {
                anyhow::bail!("cannot switch a custom Responses websocket to an official model");
            }
            proxy_custom_responses_ws(state, client_sender, body).await?;
        }

        body = loop {
            match client_receiver.next().await {
                Some(Ok(message @ (AxumWsMessage::Text(_) | AxumWsMessage::Binary(_)))) => {
                    break responses_ws_body(&message)?;
                }
                Some(Ok(AxumWsMessage::Ping(bytes))) => {
                    client_sender.send(AxumWsMessage::Pong(bytes)).await?;
                }
                Some(Ok(AxumWsMessage::Pong(_))) => {}
                Some(Ok(AxumWsMessage::Close(_))) | None => return Ok(()),
                Some(Err(err)) => return Err(err.into()),
            }
        };
        if body.get("stream").is_none() {
            body["stream"] = Value::Bool(true);
        }
    }
}

fn responses_ws_body(message: &AxumWsMessage) -> anyhow::Result<Value> {
    match message {
        AxumWsMessage::Text(text) => Ok(serde_json::from_str(text)?),
        AxumWsMessage::Binary(bytes) => Ok(serde_json::from_slice(bytes)?),
        other => anyhow::bail!(
            "first responses websocket frame must be JSON text or binary, got {other:?}"
        ),
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

fn axum_to_tungstenite_message(message: AxumWsMessage) -> Option<TungsteniteMessage> {
    match message {
        AxumWsMessage::Text(text) => Some(TungsteniteMessage::Text(text.to_string().into())),
        AxumWsMessage::Binary(bytes) => Some(TungsteniteMessage::Binary(bytes)),
        AxumWsMessage::Ping(bytes) => Some(TungsteniteMessage::Ping(bytes)),
        AxumWsMessage::Pong(bytes) => Some(TungsteniteMessage::Pong(bytes)),
        AxumWsMessage::Close(_) => Some(TungsteniteMessage::Close(None)),
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

async fn forward_official_responses(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, GatewayError> {
    let official_auth = refresh_codex_official_auth(
        &state.client,
        &state.config.codex_auth_path,
        &state.config.official_oauth_token_url,
    )
    .await
    .map_err(|err| GatewayError::Upstream(format!("Codex OAuth refresh failed: {err}")))?;
    let upstream = forward_official_headers(
        state
            .client
            .post(&state.config.official_responses_url)
            .bearer_auth(&official_auth.access_token)
            .header("chatgpt-account-id", &official_auth.account_id)
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

fn forward_official_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for name in [
        "x-codex-installation-id",
        "openai-organization",
        "openai-project",
        "user-agent",
        "accept-language",
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
    let Some(expected) = &state.config.gateway_api_key else {
        return Ok(());
    };
    let actual = bearer_token(headers);
    if actual == Some(expected.as_str()) {
        Ok(())
    } else if state.config.accept_codex_oauth
        && state.config.bind.ip().is_loopback()
        && actual.is_some()
    {
        Ok(())
    } else {
        Err(GatewayError::Unauthorized)
    }
}
