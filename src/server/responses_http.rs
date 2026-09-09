use super::auth::check_gateway_auth;
use super::*;

pub(super) async fn responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let body = super::request_body::parse_json(body).await?;
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let route = state.gateway.resolve_model_route(&requested_model).await?;
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
    let provider_routing = crate::gateway::stable_oneapi_routing(headers, &body)?;
    match route {
        ResolvedModelRoute::Official => unreachable!("official route returned above"),
        provider_route @ ResolvedModelRoute::Provider { .. } => {
            let plan = RequestPlan::from_route(provider_route, body, provider_routing, None)?;
            state.gateway.stream(plan, headers).await
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
        Ok(FusionEngine::new(state.gateway.as_ref(), &profile)
            .with_headers(headers.clone())
            .stream_with_routing(body, provider_routing))
    } else {
        body["stream"] = Value::Bool(true);
        FusionEngine::new(state.gateway.as_ref(), &profile)
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
    let sent = state.gateway.send_official(headers, body).await?;
    let status = sent.response.status();
    let content_type = sent
        .response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_owned();
    if !status.is_success() {
        let body = crate::upstream::body::read_error_text(sent.response).await?;
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
            crate::gateway::observe_upstream_cache_usage(
                sent.response.bytes_stream(),
                sent.observation,
            ),
        ))
        .map_err(|err| GatewayError::Other(err.into()))
}
