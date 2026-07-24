use super::auth::check_gateway_auth;
use super::images::{image_edits, image_generations};
use super::responses_http::responses;
use super::responses_ws::responses_ws;
use super::*;

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
                        tracing::warn!(
                            error = %format!("{error:#}"),
                            "web search capability discovery failed"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        error = %format!("{error:#}"),
                        "failed to load models for web search discovery"
                    );
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
    let targets = state.benchmark_targets(&request.providers, &request.models)?;
    let snapshot = state
        .benchmarks
        .start(targets, timeout)
        .map_err(|error| GatewayError::BadRequest(error.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(BenchmarkSnapshotResponse {
            snapshot: Some(snapshot),
        }),
    )
        .into_response())
}
