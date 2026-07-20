use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream::{self, BoxStream, SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower_http::decompression::RequestDecompressionLayer;
use uuid::Uuid;

use crate::anthropic::{BaiduAvailableModelsResponse, MessageRequest, ModelInfo, ModelsResponse};
use crate::benchmark::{BenchmarkSnapshotResponse, ModelBenchmarkManager, StartBenchmarkRequest};
use crate::catalog::{codex_catalog_from_models_with_metadata, load_template_catalog};
use crate::config::{GatewayConfig, ProviderPreset, UpstreamAuthHeader, UpstreamKind};
use crate::convert::responses_to_anthropic_with_web_search;
use crate::error::GatewayError;
use crate::image_generation::ImageRouteRegistry;
use crate::model_metadata::ModelMetadataResolver;
use crate::openai_chat::responses_to_openai_chat;
use crate::openai_events::{
    map_anthropic_sse_with_image_routes, map_openai_chat_sse_with_image_routes,
};
use crate::sse::SseDecoder;
use crate::web_search::{WebSearchCapabilities, WebSearchProbeSummary};

type OfficialWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type AnthropicByteStream = BoxStream<'static, Result<Bytes, reqwest::Error>>;
const HOSTED_WEB_SEARCH_RETRY_ATTEMPTS: usize = 3;
const MODEL_CACHE_TTL: Duration = Duration::from_secs(30);
const CATALOG_SOURCE_CACHE_TTL: Duration = Duration::from_secs(60);
const CATALOG_RESPONSE_CACHE_TTL: Duration = Duration::from_secs(30);
const ANTHROPIC_FAST_BETA: &str = "fast-mode-2026-02-01";

enum AnthropicStreamDisposition {
    Ready(AnthropicByteStream),
    RetryHostedWebSearch,
}

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
    history: Vec<Value>,
}

#[derive(Debug, Eq, PartialEq)]
struct OneApiRouting {
    session_id: String,
    hash_key: String,
}

struct CachedModels {
    fetched_at: Instant,
    models: Vec<ModelInfo>,
}

struct CatalogSources {
    template: Option<Value>,
    metadata: ModelMetadataResolver,
}

struct CachedCatalogSources {
    loaded_at: Instant,
    sources: Arc<CatalogSources>,
}

struct CachedCatalogResponse {
    generated_at: Instant,
    body: Bytes,
}

struct CachedOfficialAuth {
    modified_at: SystemTime,
    file_len: u64,
    authorization: axum::http::HeaderValue,
    account_id: axum::http::HeaderValue,
}

#[derive(Clone)]
pub struct AppState {
    config: Arc<GatewayConfig>,
    client: Client,
    image_routes: ImageRouteRegistry,
    benchmarks: ModelBenchmarkManager,
    web_search_capabilities: WebSearchCapabilities,
    models_cache: Arc<tokio::sync::Mutex<Option<CachedModels>>>,
    catalog_sources_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogSources>>>,
    catalog_response_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogResponse>>>,
    official_auth_cache: Arc<tokio::sync::Mutex<Option<CachedOfficialAuth>>>,
}

impl AppState {
    pub fn new(config: GatewayConfig) -> anyhow::Result<Self> {
        let web_search_capabilities = WebSearchCapabilities::from_default_path(&config)?;
        Self::with_web_search_capabilities(config, web_search_capabilities)
    }

    pub fn with_web_search_capabilities(
        config: GatewayConfig,
        web_search_capabilities: WebSearchCapabilities,
    ) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(64)
            .build()?;
        Ok(Self {
            config: Arc::new(config),
            client,
            image_routes: ImageRouteRegistry::default(),
            benchmarks: ModelBenchmarkManager::from_default_path(),
            web_search_capabilities,
            models_cache: Arc::new(tokio::sync::Mutex::new(None)),
            catalog_sources_cache: Arc::new(tokio::sync::Mutex::new(None)),
            catalog_response_cache: Arc::new(tokio::sync::Mutex::new(None)),
            official_auth_cache: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    fn custom_image_routes(&self) -> Option<ImageRouteRegistry> {
        self.config
            .upstream_image_generation_path
            .is_some()
            .then(|| self.image_routes.clone())
    }

    pub async fn fetch_models(&self) -> Result<Vec<ModelInfo>, GatewayError> {
        let mut cache = self.models_cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.fetched_at.elapsed() < MODEL_CACHE_TTL)
        {
            let mut models = cached.models.clone();
            drop(cache);
            self.web_search_capabilities.annotate_models(&mut models);
            return Ok(models);
        }

        let mut models = self.fetch_models_uncached().await?;
        *cache = Some(CachedModels {
            fetched_at: Instant::now(),
            models: models.clone(),
        });
        drop(cache);
        self.web_search_capabilities.annotate_models(&mut models);
        Ok(models)
    }

    async fn fetch_models_uncached(&self) -> Result<Vec<ModelInfo>, GatewayError> {
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

    async fn catalog_sources(&self) -> Result<Arc<CatalogSources>, GatewayError> {
        let mut cache = self.catalog_sources_cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.loaded_at.elapsed() < CATALOG_SOURCE_CACHE_TTL)
        {
            return Ok(Arc::clone(&cached.sources));
        }
        let sources = tokio::task::spawn_blocking(|| -> anyhow::Result<CatalogSources> {
            Ok(CatalogSources {
                template: load_template_catalog(None)?,
                metadata: ModelMetadataResolver::from_default_files()?,
            })
        })
        .await
        .map_err(|error| anyhow::anyhow!("catalog source loader failed: {error}"))??;
        let sources = Arc::new(sources);
        *cache = Some(CachedCatalogSources {
            loaded_at: Instant::now(),
            sources: Arc::clone(&sources),
        });
        Ok(sources)
    }

    async fn catalog_response(&self) -> Result<Bytes, GatewayError> {
        let mut cache = self.catalog_response_cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.generated_at.elapsed() < CATALOG_RESPONSE_CACHE_TTL)
        {
            return Ok(cached.body.clone());
        }

        let models = self.fetch_models().await?;
        let sources = self.catalog_sources().await?;
        let default_context_window = self.config.default_context_window;
        let body = tokio::task::spawn_blocking(move || {
            let catalog = codex_catalog_from_models_with_metadata(
                &models,
                default_context_window,
                sources.template.as_ref(),
                &sources.metadata,
            );
            serde_json::to_vec(&catalog).map(Bytes::from)
        })
        .await
        .map_err(|error| anyhow::anyhow!("catalog response generator failed: {error}"))??;
        *cache = Some(CachedCatalogResponse {
            generated_at: Instant::now(),
            body: body.clone(),
        });
        Ok(body)
    }

    async fn official_auth(
        &self,
    ) -> anyhow::Result<(axum::http::HeaderValue, axum::http::HeaderValue)> {
        read_codex_official_auth(&self.config.codex_auth_path, &self.official_auth_cache).await
    }

    pub async fn probe_web_search_capabilities(
        &self,
        models: &mut [crate::anthropic::ModelInfo],
        force: bool,
    ) -> anyhow::Result<WebSearchProbeSummary> {
        self.web_search_capabilities
            .probe_models(models, &self.config, force)
            .await
    }

    fn web_search_enabled_for_custom_request(&self, body: &Value) -> bool {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.config.enable_web_search_tool && self.web_search_capabilities.supports_model(model)
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
        request.header("anthropic-version", &self.config.anthropic_version)
    }

    fn apply_oneapi_affinity(
        &self,
        request: reqwest::RequestBuilder,
        hash_key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        if let Some(hash_key) = hash_key {
            request.header("x-hash-key", hash_key)
        } else {
            request
        }
    }

    async fn send_anthropic_request(
        &self,
        request: &MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let mut upstream_request =
            self.apply_upstream_auth(self.client.post(self.config.upstream_messages_url()));
        let beta = if request.speed.as_deref() == Some("fast") {
            Some(match self.config.anthropic_beta.as_deref() {
                Some(configured)
                    if configured
                        .split(',')
                        .any(|item| item.trim() == ANTHROPIC_FAST_BETA) =>
                {
                    configured.to_owned()
                }
                Some(configured) if !configured.trim().is_empty() => {
                    format!("{configured},{ANTHROPIC_FAST_BETA}")
                }
                _ => ANTHROPIC_FAST_BETA.to_owned(),
            })
        } else {
            self.config.anthropic_beta.clone()
        };
        if let Some(beta) = beta {
            upstream_request = upstream_request.header("anthropic-beta", beta);
        }
        let response = self
            .apply_oneapi_affinity(upstream_request, hash_key)
            .header(header::ACCEPT, "text/event-stream")
            .json(request)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(GatewayError::Upstream(format!(
                "messages endpoint returned {status}: {body}"
            )));
        }
        Ok(response.bytes_stream().boxed())
    }

    async fn anthropic_stream_with_web_search_retry(
        &self,
        mut request: MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let has_hosted_web_search = request.tools.iter().any(|tool| {
            tool.get("name").and_then(Value::as_str) == Some("web_search")
                && tool
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|tool_type| tool_type.starts_with("web_search_"))
        });
        let upstream = self.send_anthropic_request(&request, hash_key).await?;
        if !has_hosted_web_search {
            return Ok(upstream);
        }
        match inspect_anthropic_stream(upstream).await? {
            AnthropicStreamDisposition::Ready(upstream) => Ok(upstream),
            AnthropicStreamDisposition::RetryHostedWebSearch => {
                tracing::warn!(
                    model = %request.model,
                    "retrying client-style web_search call as an Anthropic server tool"
                );
                request.tool_choice = Some(json!({"type":"tool","name":"web_search"}));
                let original_session_id = request
                    .metadata
                    .as_ref()
                    .and_then(Value::as_object)
                    .and_then(|metadata| metadata.get("session_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                for attempt in 1..=HOSTED_WEB_SEARCH_RETRY_ATTEMPTS {
                    if let Some(metadata) = request.metadata.as_mut().and_then(Value::as_object_mut)
                    {
                        metadata.insert(
                            "session_id".to_owned(),
                            json!(format!(
                                "{}-web-search-retry-{}",
                                original_session_id.as_deref().unwrap_or("codex-mixin"),
                                Uuid::new_v4().simple()
                            )),
                        );
                    }
                    let retry = self.send_anthropic_request(&request, hash_key).await?;
                    match inspect_anthropic_stream(retry).await? {
                        AnthropicStreamDisposition::Ready(retry) => return Ok(retry),
                        AnthropicStreamDisposition::RetryHostedWebSearch => tracing::warn!(
                            model = %request.model,
                            attempt,
                            "forced hosted web_search was still returned as a client tool"
                        ),
                    }
                }
                Err(GatewayError::Upstream(format!(
                    "model {} returned a client-style web_search call after {} hosted-tool retries",
                    request.model, HOSTED_WEB_SEARCH_RETRY_ATTEMPTS
                )))
            }
        }
    }
}

async fn inspect_anthropic_stream(
    mut upstream: AnthropicByteStream,
) -> Result<AnthropicStreamDisposition, GatewayError> {
    let mut buffered_chunks = Vec::new();
    let mut decoder = SseDecoder::default();
    while let Some(chunk) = upstream.next().await {
        let chunk = chunk?;
        let events = decoder.push(&chunk);
        buffered_chunks.push(chunk);
        let mut retry_hosted_web_search = None;
        for event in events {
            if event.data == "[DONE]" {
                retry_hosted_web_search = Some(false);
                break;
            }
            let Ok(payload) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            match payload.get("type").and_then(Value::as_str) {
                Some("content_block_start") => {
                    let block = payload.get("content_block").unwrap_or(&Value::Null);
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            retry_hosted_web_search = Some(
                                block.get("name").and_then(Value::as_str) == Some("web_search"),
                            );
                        }
                        Some("server_tool_use") => retry_hosted_web_search = Some(false),
                        _ => {}
                    }
                }
                Some("content_block_delta") => {
                    let delta = payload.get("delta").unwrap_or(&Value::Null);
                    if delta.get("type").and_then(Value::as_str) == Some("text_delta")
                        && delta
                            .get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| !text.is_empty())
                    {
                        retry_hosted_web_search = Some(false);
                    }
                }
                Some("message_stop" | "error") => retry_hosted_web_search = Some(false),
                _ => {}
            }
            if retry_hosted_web_search.is_some() {
                break;
            }
        }
        if let Some(retry_hosted_web_search) = retry_hosted_web_search {
            if retry_hosted_web_search {
                return Ok(AnthropicStreamDisposition::RetryHostedWebSearch);
            }
            let prefix = stream::iter(buffered_chunks.into_iter().map(Ok));
            return Ok(AnthropicStreamDisposition::Ready(
                prefix.chain(upstream).boxed(),
            ));
        }
    }
    Ok(AnthropicStreamDisposition::Ready(
        stream::iter(buffered_chunks.into_iter().map(Ok)).boxed(),
    ))
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
    let probe_state = state.clone();
    let probe_task = state.config.enable_web_search_tool.then(|| {
        tokio::spawn(async move {
            match probe_state.fetch_models().await {
                Ok(mut models) => {
                    if let Err(error) = probe_state
                        .probe_web_search_capabilities(&mut models, false)
                        .await
                    {
                        tracing::warn!(error = %error, "web search capability discovery failed");
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "failed to load models for web search discovery");
                }
            }
        })
    });
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tracing::info!(%bind, "codex-mixin listening");
    let result = axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    if let Some(probe_task) = probe_task {
        probe_task.abort();
    }
    result?;
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
    let body = state.catalog_response().await?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .map_err(|error| GatewayError::Other(error.into()))
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
    let oneapi_routing = if state.config.provider_preset == ProviderPreset::BaiduOneApi {
        stable_oneapi_routing(&headers, &body)?
    } else {
        None
    };
    let stream = match state.config.upstream_kind {
        UpstreamKind::AnthropicMessages => {
            let mut converted = responses_to_anthropic_with_web_search(
                &body,
                &state.config,
                state.web_search_enabled_for_custom_request(&body),
            )?;
            if let Some(routing) = &oneapi_routing {
                converted.request.metadata = Some(json!({"session_id": routing.session_id}));
            }
            let upstream = state
                .anthropic_stream_with_web_search_retry(
                    converted.request,
                    oneapi_routing
                        .as_ref()
                        .map(|routing| routing.hash_key.as_str()),
                )
                .await?;
            Body::from_stream(map_anthropic_sse_with_image_routes(
                upstream,
                body,
                converted.tool_names,
                state.custom_image_routes(),
            ))
        }
        UpstreamKind::OpenAiChat => {
            let converted = responses_to_openai_chat(&body)?;
            let upstream_request =
                state.apply_upstream_auth(state.client.post(state.config.upstream_messages_url()));
            let upstream = state
                .apply_oneapi_affinity(
                    upstream_request,
                    oneapi_routing
                        .as_ref()
                        .map(|routing| routing.hash_key.as_str()),
                )
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

        if should_forward_to_official(&body) {
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
        let next_state = match expand_custom_websocket_history(&mut body, custom_state.take()) {
            Ok(()) if is_noop_responses_ws_request(&body) => {
                complete_custom_noop(&mut client_sender, body)
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
    normalize_custom_model_alias(&mut body);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("custom request is missing model"))?
        .to_owned();
    if !body.get("input").is_some_and(Value::is_array) {
        anyhow::bail!("custom request input must be an array");
    }
    let oneapi_routing = if state.config.provider_preset == ProviderPreset::BaiduOneApi {
        stable_oneapi_routing(headers, &body)?
    } else {
        None
    };
    let (stream, mut history): (
        futures_util::stream::BoxStream<'static, Result<bytes::Bytes, std::convert::Infallible>>,
        Vec<Value>,
    ) = match state.config.upstream_kind {
        UpstreamKind::AnthropicMessages => {
            let mut converted = responses_to_anthropic_with_web_search(
                &body,
                &state.config,
                state.web_search_enabled_for_custom_request(&body),
            )?;
            if let Some(routing) = &oneapi_routing {
                converted.request.metadata = Some(json!({"session_id": routing.session_id}));
            }
            let upstream = state
                .anthropic_stream_with_web_search_retry(
                    converted.request,
                    oneapi_routing
                        .as_ref()
                        .map(|routing| routing.hash_key.as_str()),
                )
                .await?;
            let history = take_custom_request_input(&mut body)?;
            (
                map_anthropic_sse_with_image_routes(
                    upstream,
                    body,
                    converted.tool_names,
                    state.custom_image_routes(),
                )
                .boxed(),
                history,
            )
        }
        UpstreamKind::OpenAiChat => {
            let converted = responses_to_openai_chat(&body)?;
            let upstream_request =
                state.apply_upstream_auth(state.client.post(state.config.upstream_messages_url()));
            let upstream = state
                .apply_oneapi_affinity(
                    upstream_request,
                    oneapi_routing
                        .as_ref()
                        .map(|routing| routing.hash_key.as_str()),
                )
                .header(header::ACCEPT, "text/event-stream")
                .json(&converted.request)
                .send()
                .await?;
            let status = upstream.status();
            if !status.is_success() {
                let body = upstream.text().await?;
                anyhow::bail!("chat completions endpoint returned {status}: {body}");
            }
            let history = take_custom_request_input(&mut body)?;
            (
                map_openai_chat_sse_with_image_routes(
                    upstream.bytes_stream(),
                    body,
                    converted.tool_names,
                    state.custom_image_routes(),
                )
                .boxed(),
                history,
            )
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
    let mut full_input = state.history;
    full_input.extend(incremental_input.iter().cloned());
    body["input"] = Value::Array(full_input);
    Ok(())
}

async fn complete_custom_noop(
    client_sender: &mut SplitSink<WebSocket, AxumWsMessage>,
    mut body: Value,
) -> anyhow::Result<CustomWebSocketState> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("custom noop request is missing model"))?;
    let model = model.strip_suffix("-custom").unwrap_or(model).to_owned();
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
    let (authorization, account_id) = state.official_auth().await.map_err(GatewayError::Other)?;
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
    let (authorization, account_id) = state.official_auth().await.map_err(GatewayError::Other)?;
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

async fn read_codex_official_auth(
    auth_path: &std::path::Path,
    cache: &tokio::sync::Mutex<Option<CachedOfficialAuth>>,
) -> anyhow::Result<(axum::http::HeaderValue, axum::http::HeaderValue)> {
    let metadata = tokio::fs::metadata(auth_path).await.map_err(|err| {
        anyhow::anyhow!("read Codex auth metadata {}: {err}", auth_path.display())
    })?;
    let modified_at = metadata.modified().map_err(|err| {
        anyhow::anyhow!(
            "read Codex auth modification time {}: {err}",
            auth_path.display()
        )
    })?;
    let mut cache = cache.lock().await;
    if let Some(cached) = cache
        .as_ref()
        .filter(|cached| cached.modified_at == modified_at && cached.file_len == metadata.len())
    {
        return Ok((cached.authorization.clone(), cached.account_id.clone()));
    }

    let raw = tokio::fs::read_to_string(auth_path)
        .await
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
    let authorization: axum::http::HeaderValue = format!("Bearer {access_token}").parse()?;
    let account_id: axum::http::HeaderValue = account_id.parse()?;
    *cache = Some(CachedOfficialAuth {
        modified_at,
        file_len: metadata.len(),
        authorization: authorization.clone(),
        account_id: account_id.clone(),
    });
    Ok((authorization, account_id))
}

fn forward_official_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
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
    model
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("gpt-"))
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

fn stable_oneapi_routing(
    headers: &HeaderMap,
    body: &Value,
) -> Result<Option<OneApiRouting>, GatewayError> {
    let mut route_key = None;
    for header_name in ["session-id", "thread-id", "x-client-request-id"] {
        if let Some(value) = headers.get(header_name) {
            let value = value.to_str().map_err(|error| {
                GatewayError::BadRequest(format!("invalid {header_name} header: {error}"))
            })?;
            if !value.is_empty() {
                route_key = Some(value);
                break;
            }
        }
    }
    if route_key.is_none() {
        match body.get("prompt_cache_key") {
            None | Some(Value::Null) => {}
            Some(Value::String(prompt_cache_key)) if !prompt_cache_key.is_empty() => {
                route_key = Some(prompt_cache_key);
            }
            Some(Value::String(_)) => {}
            Some(_) => {
                return Err(GatewayError::BadRequest(
                    "prompt_cache_key must be a string".to_owned(),
                ));
            }
        }
    }
    Ok(route_key.map(|session_id| OneApiRouting {
        session_id: session_id.to_owned(),
        hash_key: Uuid::new_v5(&Uuid::NAMESPACE_URL, session_id.as_bytes()).to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use bytes::Bytes;

    use super::*;
    use crate::benchmark::ModelBenchmarkManager;
    use crate::config::{ThinkingMode, UpstreamAuthHeader};

    #[test]
    fn oneapi_routing_uses_stable_identifier_priority() {
        let mut headers = HeaderMap::new();
        headers.insert("session-id", "session-value".parse().unwrap());
        headers.insert("thread-id", "thread-value".parse().unwrap());
        headers.insert("x-client-request-id", "request-value".parse().unwrap());
        let body = json!({"prompt_cache_key":"cache-value"});

        let routing = stable_oneapi_routing(&headers, &body).unwrap().unwrap();
        assert_eq!(routing.session_id, "session-value");
        assert_eq!(
            routing.hash_key,
            Uuid::new_v5(&Uuid::NAMESPACE_URL, b"session-value").to_string()
        );

        headers.remove("session-id");
        assert_eq!(
            stable_oneapi_routing(&headers, &body)
                .unwrap()
                .unwrap()
                .session_id,
            "thread-value"
        );
        headers.remove("thread-id");
        assert_eq!(
            stable_oneapi_routing(&headers, &body)
                .unwrap()
                .unwrap()
                .session_id,
            "request-value"
        );
        headers.clear();
        assert_eq!(
            stable_oneapi_routing(&headers, &body)
                .unwrap()
                .unwrap()
                .session_id,
            "cache-value"
        );
        assert!(
            stable_oneapi_routing(&headers, &json!({}))
                .unwrap()
                .is_none()
        );
        assert!(
            stable_oneapi_routing(&headers, &json!({"prompt_cache_key":null}))
                .unwrap()
                .is_none()
        );
        assert!(stable_oneapi_routing(&headers, &json!({"prompt_cache_key":1})).is_err());
    }

    #[tokio::test]
    async fn official_auth_cache_refreshes_and_does_not_hide_invalid_files() {
        let directory = tempfile::tempdir().unwrap();
        let auth_path = directory.path().join("auth.json");
        let cache = tokio::sync::Mutex::new(None);
        tokio::fs::write(
            &auth_path,
            r#"{"tokens":{"access_token":"first","account_id":"account-one"}}"#,
        )
        .await
        .unwrap();

        let (authorization, account_id) =
            read_codex_official_auth(&auth_path, &cache).await.unwrap();
        assert_eq!(authorization, "Bearer first");
        assert_eq!(account_id, "account-one");

        tokio::fs::write(
            &auth_path,
            r#"{"tokens":{"access_token":"second-longer","account_id":"account-two"}}"#,
        )
        .await
        .unwrap();
        let (authorization, account_id) =
            read_codex_official_auth(&auth_path, &cache).await.unwrap();
        assert_eq!(authorization, "Bearer second-longer");
        assert_eq!(account_id, "account-two");

        tokio::fs::write(&auth_path, b"{").await.unwrap();
        assert!(read_codex_official_auth(&auth_path, &cache).await.is_err());
    }

    #[tokio::test]
    async fn benchmark_api_runs_after_the_start_request_returns_and_persists_results() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);
        let model_requests = Arc::new(AtomicUsize::new(0));
        let captured_model_requests = Arc::clone(&model_requests);
        let upstream = Router::new()
            .route(
                "/v1/models",
                get(move || {
                    let captured_model_requests = Arc::clone(&captured_model_requests);
                    async move {
                        captured_model_requests.fetch_add(1, Ordering::Relaxed);
                        Json(json!({
                            "object":"list",
                            "data":[{"id":"benchmark-model","object":"model"}]
                        }))
                    }
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
        })
        .unwrap();
        state.benchmarks = ModelBenchmarkManager::new(results_path.clone());
        tokio::spawn(async move {
            axum::serve(gateway_listener, router(state)).await.unwrap();
        });

        let client = Client::new();
        for _ in 0..2 {
            client
                .get(format!("http://{gateway_address}/v1/models"))
                .bearer_auth("gateway-key")
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
        }
        assert_eq!(model_requests.load(Ordering::Relaxed), 1);
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
