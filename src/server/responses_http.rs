use super::auth::{check_gateway_auth, forward_official_headers, stable_oneapi_routing};
use super::*;

pub(super) async fn responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let body = crate::protocol::request_body::parse_json(body).await?;
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let route = state.resolve_model_route(&requested_model).await?;
    log_responses_route(&requested_model, &route);
    if route == ResolvedModelRoute::Official {
        let (body, _) = crate::images::normalize_provider_images_blocking(body).await?;
        return forward_official_responses(&state, &headers, body).await;
    }
    let stream = stream_custom_responses(&state, &headers, body, route).await?;
    sse_response(stream)
}

fn log_responses_route(requested_model: &str, route: &ResolvedModelRoute) {
    match &route {
        ResolvedModelRoute::Official => {
            tracing::info!(catalog_slug = %requested_model, route = "official", "routing responses request");
        }
        ResolvedModelRoute::Fusion { profile_id } => {
            tracing::info!(
                catalog_slug = %requested_model,
                fusion_profile_id = %profile_id,
                route = "fusion",
                "routing responses request"
            );
        }
        ResolvedModelRoute::Provider {
            provider_id,
            upstream_model_id,
            ..
        } => {
            tracing::info!(
                catalog_slug = %requested_model,
                provider_id = %provider_id,
                upstream_model_id = %upstream_model_id,
                route = "provider",
                "routing responses request"
            );
        }
    }
}

async fn stream_custom_responses(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
    route: ResolvedModelRoute,
) -> Result<ResponseStream, GatewayError> {
    let provider_routing = stable_oneapi_routing(headers, &body)?;
    match route {
        ResolvedModelRoute::Official => unreachable!("official route returned above"),
        provider_route @ ResolvedModelRoute::Provider { .. } => {
            let plan = RequestPlan::from_route(provider_route, body, provider_routing, None)?;
            UpstreamExecutor::new(state).stream(plan, headers).await
        }
        ResolvedModelRoute::Fusion { profile_id } => {
            stream_fusion_responses(state, headers, body, provider_routing, profile_id).await
        }
    }
}

async fn stream_fusion_responses(
    state: &AppState,
    headers: &HeaderMap,
    mut body: Value,
    provider_routing: Option<crate::gateway::UpstreamRouting>,
    profile_id: String,
) -> Result<ResponseStream, GatewayError> {
    let profile = state
        .config
        .fusion_profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| GatewayError::BadRequest(format!("unknown fusion profile: {profile_id}")))?
        .clone();
    if should_fuse_turn(&body) {
        Ok(FusionEngine::new(state, &profile)
            .with_headers(headers.clone())
            .stream_with_routing(body, provider_routing))
    } else {
        body["stream"] = Value::Bool(true);
        FusionEngine::new(state, &profile)
            .with_headers(headers.clone())
            .stream_final_continuation(body, provider_routing.as_ref())
            .await
    }
}

fn sse_response(stream: ResponseStream) -> Result<Response, GatewayError> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .map_err(|err| GatewayError::Other(err.into()))
}

async fn forward_official_responses(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, GatewayError> {
    let observation = official_prefix_observation(state, headers, &body)?;
    let (authorization, account_id) = state.official_auth().await.map_err(GatewayError::Other)?;
    let mut upstream = send_official_responses(
        state,
        headers,
        body.clone(),
        authorization.clone(),
        account_id.clone(),
    )
    .await?;
    if upstream.status() == StatusCode::PAYLOAD_TOO_LARGE {
        let (fallback_body, stats) =
            crate::images::normalize_provider_images_for_fallback(body).await?;
        if stats.normalized_images > 0 && stats.saved_bytes > 0 {
            tracing::warn!(
                normalized_images = stats.normalized_images,
                saved_image_bytes = stats.saved_bytes,
                "retrying official responses request after 413 with aggressively compressed images"
            );
            upstream =
                send_official_responses(state, headers, fallback_body, authorization, account_id)
                    .await?;
        }
    }
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_owned();
    if !status.is_success() {
        let body = crate::protocol::request_body::read_error_text(upstream).await?;
        return Err(GatewayError::UpstreamStatus {
            status,
            message: format!("official responses endpoint returned {status}: {body}"),
        });
    }
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(
            crate::gateway::observe_upstream_cache_usage(upstream.bytes_stream(), observation),
        ))
        .map_err(|err| GatewayError::Other(err.into()))
}

async fn send_official_responses(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
    authorization: axum::http::HeaderValue,
    account_id: axum::http::HeaderValue,
) -> Result<reqwest::Response, GatewayError> {
    let body = normalize_official_responses_body(body);
    let request = forward_official_headers(
        state
            .client
            .post(&state.config.official_responses_url)
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "text/event-stream"),
        headers,
    );
    crate::protocol::request_body::send_json(request, body).await
}

pub(super) fn normalize_official_responses_body(mut body: Value) -> Value {
    if let Some(body) = body.as_object_mut() {
        body.remove("max_output_tokens");
    }
    body
}

pub(crate) async fn stream_official_response(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
) -> Result<ResponseStream, GatewayError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("official")
        .to_owned();
    let observation = official_prefix_observation(state, headers, body)?;
    let (authorization, account_id) = state.official_auth().await.map_err(GatewayError::Other)?;
    let mut upstream = send_official_responses(
        state,
        headers,
        body.clone(),
        authorization.clone(),
        account_id.clone(),
    )
    .await?;
    if upstream.status() == StatusCode::PAYLOAD_TOO_LARGE {
        let (fallback_body, stats) =
            crate::images::normalize_provider_images_for_fallback(body.clone()).await?;
        if stats.normalized_images > 0 && stats.saved_bytes > 0 {
            tracing::warn!(
                normalized_images = stats.normalized_images,
                saved_image_bytes = stats.saved_bytes,
                "retrying official responses stream after 413 with aggressively compressed images"
            );
            upstream =
                send_official_responses(state, headers, fallback_body, authorization, account_id)
                    .await?;
        }
    }
    let status = upstream.status();
    if !status.is_success() {
        let body = crate::protocol::request_body::read_error_text(upstream).await?;
        return Err(GatewayError::UpstreamStatus {
            status,
            message: format!("official responses endpoint returned {status}: {body}"),
        });
    }
    let stream = async_stream::stream! {
        let upstream = crate::gateway::observe_upstream_cache_usage(
            upstream.bytes_stream(),
            observation,
        );
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(bytes) => yield Ok::<Bytes, Infallible>(bytes),
                Err(error) => {
                    let event = encode_event(
                        "response.failed",
                        &crate::protocol::sse::response_failed_payload(
                            None,
                            Some(&model),
                            error.to_string(),
                            "server_error",
                        ),
                    )
                    .expect("official failure event is serializable");
                    yield Ok(event);
                    break;
                }
            }
        }
    };
    Ok(stream.boxed())
}

pub(super) fn official_prefix_observation(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
) -> Result<Option<crate::gateway::PrefixObservation>, GatewayError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?;
    let routing = stable_oneapi_routing(headers, body)?;
    Ok(crate::gateway::record_provider_prefix(
        &state.cache_shapes,
        "official",
        model,
        model,
        routing.as_ref(),
        crate::gateway::CacheShape::from_openai_responses(body),
    ))
}
