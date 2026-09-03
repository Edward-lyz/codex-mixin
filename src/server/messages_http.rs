use super::auth::{check_gateway_auth, stable_oneapi_routing};
use super::*;
use crate::anthropic::{ContentBlock, Message, MessageRequest};
use crate::provider::ProviderProtocol;
use async_stream::stream;

pub(super) async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let body = crate::protocol::request_body::parse_json(body).await?;
    let (body, _) = crate::images::normalize_anthropic_images_blocking(
        body,
        crate::images::ImageCompressionProfile::Primary,
    )
    .await?;
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?;
    let route = state.resolve_model_route(requested_model).await?;
    if route == ResolvedModelRoute::Official {
        let request = normalize_message_request(&body, requested_model)?;
        let responses_body =
            super::anthropic_compat::message_request_to_responses(&request, requested_model)?;
        let plan = RequestPlan::official(responses_body, Some(requested_model.to_owned()))?;
        return responses_compatible_message(
            &state,
            &headers,
            requested_model,
            body_stream_requested(&body),
            plan,
        )
        .await;
    }
    let resolved = state.resolve_native_provider_model(requested_model)?;
    let provider = resolved.provider;
    let upstream_model_id = resolved.upstream_model_id;
    super::auth::require_ducx_client(&state, provider, &headers)?;
    let request = normalize_message_request(&body, upstream_model_id)?;
    let routing = stable_oneapi_routing(&headers, &body)?;
    let stream_requested = body_stream_requested(&body);
    if provider.protocol_for_model(upstream_model_id) != ProviderProtocol::AnthropicMessages {
        let responses_body =
            super::anthropic_compat::message_request_to_responses(&request, requested_model)?;
        let plan = RequestPlan::provider(
            resolved.catalog_slug.to_owned(),
            provider.id().to_owned(),
            upstream_model_id.to_owned(),
            responses_body,
            routing,
            Some(requested_model.to_owned()),
        )?;
        return responses_compatible_message(
            &state,
            &headers,
            requested_model,
            stream_requested,
            plan,
        )
        .await;
    }
    let hash_key = routing.map(|routing| routing.hash_key);
    let first = state
        .anthropic_stream_with_web_search_retry(provider, request, hash_key.as_deref())
        .await;
    let upstream = match first {
        Ok(upstream) => upstream,
        Err(error @ GatewayError::UpstreamStatus { status, .. })
            if status == StatusCode::PAYLOAD_TOO_LARGE =>
        {
            let (fallback_body, stats) = crate::images::normalize_anthropic_images_blocking(
                body,
                crate::images::ImageCompressionProfile::PayloadFallback,
            )
            .await?;
            if stats.normalized_images == 0 || stats.saved_bytes == 0 {
                return Err(error);
            }
            tracing::warn!(
                provider_id = provider.id(),
                upstream_model_id,
                normalized_images = stats.normalized_images,
                saved_image_bytes = stats.saved_bytes,
                "retrying Anthropic Messages request after 413 with aggressively compressed images"
            );
            let fallback_request = normalize_message_request(&fallback_body, upstream_model_id)?;
            state
                .anthropic_stream_with_web_search_retry(
                    provider,
                    fallback_request,
                    hash_key.as_deref(),
                )
                .await?
        }
        Err(error) => return Err(error),
    };
    let stream = relay_anthropic_stream(upstream);
    let body = Body::from_stream(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            if stream_requested {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .map_err(|err| GatewayError::Other(err.into()))
}

async fn responses_compatible_message(
    state: &AppState,
    headers: &HeaderMap,
    requested_model: &str,
    stream_requested: bool,
    plan: RequestPlan,
) -> Result<Response, GatewayError> {
    let upstream = UpstreamExecutor::new(state).stream(plan, headers).await?;
    if stream_requested {
        let stream = super::anthropic_compat::responses_to_anthropic_stream(
            upstream,
            requested_model.to_owned(),
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(stream))
            .map_err(|err| GatewayError::Other(err.into()));
    }
    let collected = crate::gateway::collect_response_stream(upstream).await?;
    let message =
        super::anthropic_compat::collected_to_anthropic_message(collected, requested_model)?;
    Ok(Json(message).into_response())
}

fn body_stream_requested(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool) == Some(true)
}

pub(super) fn normalize_message_request(
    body: &Value,
    upstream_model: &str,
) -> Result<MessageRequest, GatewayError> {
    let max_tokens = body
        .get("max_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| GatewayError::BadRequest("missing max_tokens".to_owned()))?;
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| GatewayError::BadRequest("missing messages".to_owned()))?;
    let messages = messages
        .iter()
        .map(normalize_message)
        .collect::<Result<Vec<_>, GatewayError>>()?;
    let system = match body.get("system") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(vec![ContentBlock::Text { text: text.clone() }]),
        Some(Value::Array(blocks)) => Some(
            blocks
                .iter()
                .map(normalize_content_block)
                .collect::<Result<Vec<_>, GatewayError>>()?,
        ),
        Some(_) => {
            return Err(GatewayError::BadRequest(
                "system must be a string or array".to_owned(),
            ));
        }
    };
    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(MessageRequest {
        model: upstream_model.to_owned(),
        max_tokens,
        stream: body_stream_requested(body),
        speed: body.get("speed").and_then(Value::as_str).map(str::to_owned),
        messages,
        system,
        tools,
        tool_choice: body.get("tool_choice").cloned(),
        thinking: body.get("thinking").cloned(),
        output_config: body.get("output_config").cloned(),
        metadata: body.get("metadata").cloned(),
    })
}

fn normalize_message(message: &Value) -> Result<Message, GatewayError> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("message missing role".to_owned()))?
        .to_owned();
    let content = match message.get("content") {
        Some(Value::String(text)) => vec![ContentBlock::Text { text: text.clone() }],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(normalize_content_block)
            .collect::<Result<Vec<_>, GatewayError>>()?,
        Some(_) => {
            return Err(GatewayError::BadRequest(
                "message content must be a string or array".to_owned(),
            ));
        }
        None => {
            return Err(GatewayError::BadRequest(
                "message missing content".to_owned(),
            ));
        }
    };
    Ok(Message { role, content })
}

fn normalize_content_block(block: &Value) -> Result<ContentBlock, GatewayError> {
    serde_json::from_value(block.clone()).map_err(|error| {
        GatewayError::BadRequest(format!("invalid Anthropic content block: {error}"))
    })
}

fn relay_anthropic_stream(
    upstream: AnthropicByteStream,
) -> BoxStream<'static, Result<Bytes, Infallible>> {
    stream! {
        let mut upstream = upstream;
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(bytes) => yield Ok(bytes),
                Err(error) => {
                    let error = json!({
                        "type": "error",
                        "error": {
                            "type": "api_error",
                            "message": error.to_string()
                        }
                    });
                    if let Ok(event) = encode_event("error", &error) {
                        yield Ok(event);
                    }
                    return;
                }
            }
        }
    }
    .boxed()
}
