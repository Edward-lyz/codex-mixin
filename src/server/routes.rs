use super::auth::check_gateway_auth;
use super::images::{image_edits, image_generations};
use super::realtime::{live_sideband_ws, live_ws, realtime_call, realtime_ws};
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
        .route("/v1/usage", get(token_usage))
        .route("/v1/responses", get(responses_ws).post(responses))
        .route("/v1/messages", post(super::messages_http::messages))
        .route("/v1/realtime", get(realtime_ws))
        .route("/v1/realtime/calls", post(realtime_call))
        .route("/v1/live", get(live_ws).post(realtime_call))
        .route("/v1/live/{call_id}", get(live_sideband_ws))
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
    let ducc_prewarm_state = state.clone();
    let ducc_prewarm_task = tokio::spawn(async move {
        if let Err(error) = ducc_prewarm_state.prewarm_ducc().await {
            tracing::warn!(
                error = %format!("{error:#}"),
                "managed DUCC authentication header prewarm failed"
            );
        }
    });
    let ducx_prewarm_state = state.clone();
    let ducx_prewarm_task = tokio::spawn(async move {
        if let Err(error) = ducx_prewarm_state.prewarm_ducx().await {
            tracing::warn!(
                error = %format!("{error:#}"),
                "managed DUCX authentication header prewarm failed"
            );
        }
    });
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
    ducc_prewarm_task.abort();
    ducx_prewarm_task.abort();
    result?;
    Ok(())
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let mut healthy = 0;
    let mut degraded = 0;
    for provider in state.providers.providers() {
        match provider.definition().readiness().status {
            crate::provider::ProviderReadinessStatus::Healthy => healthy += 1,
            crate::provider::ProviderReadinessStatus::Degraded => degraded += 1,
            crate::provider::ProviderReadinessStatus::Disabled => {}
        }
    }
    let provider_readiness = if degraded > 0 {
        "degraded"
    } else if healthy > 0 {
        "healthy"
    } else {
        "disabled"
    };
    Json(json!({
        "ok": true,
        "provider_readiness": provider_readiness,
    }))
}

async fn models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let models = state.fetch_models().await?;
    Ok(Json(json!({"object":"list","data":models})).into_response())
}

async fn codex_model_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
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
    check_gateway_auth(&state, &headers).await?;
    let snapshot = state.benchmarks.snapshot().map_err(GatewayError::Other)?;
    Ok(Json(BenchmarkSnapshotResponse { snapshot }).into_response())
}

async fn start_model_benchmarks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<StartBenchmarkRequest>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let timeout = std::time::Duration::from_secs(request.timeout_seconds);
    if timeout.is_zero() || timeout > std::time::Duration::from_secs(300) {
        return Err(GatewayError::BadRequest(
            "model benchmark timeout must be between 1 and 300 seconds".to_owned(),
        ));
    }
    let targets = state.benchmark_targets(&request.providers, &request.models)?;
    let snapshot = state
        .benchmarks
        .start(targets, timeout, request.target_output_tokens)
        .map_err(|error| GatewayError::BadRequest(error.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(BenchmarkSnapshotResponse {
            snapshot: Some(snapshot),
        }),
    )
        .into_response())
}

async fn token_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ProviderTokenUsage>>, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    Ok(Json(state.cache_shapes.usage_snapshot()))
}
