use super::auth::{check_gateway_auth, stable_oneapi_routing};
use super::*;
use crate::anthropic::{ContentBlock, Message, MessageRequest};
use crate::provider::ProviderProtocol;
use async_stream::stream;
use serde::Deserialize;

pub(super) async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let body = super::request_body::parse_json(body).await?;
    messages_body(state, headers, body).await
}

pub(super) async fn messages_body(
    state: AppState,
    headers: HeaderMap,
    body: Value,
) -> Result<Response, GatewayError> {
    let (mut body, _) = crate::images::normalize_anthropic_images_blocking(
        body,
        crate::images::ImageCompressionProfile::Primary,
    )
    .await?;
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let requested_model = requested_model.as_str();
    let route = state.gateway.resolve_model_route(requested_model).await?;
    if route == ResolvedModelRoute::Official {
        let request = normalize_message_request(&body, requested_model, false)?;
        let responses_body = crate::protocol::anthropic_compat::message_request_to_responses(
            &request,
            requested_model,
        )?;
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
    if matches!(route, ResolvedModelRoute::Fusion { .. })
        || requested_model.starts_with(crate::config::FUSION_MODEL_PREFIX)
    {
        // Routes speak OpenAI Responses; translate Claude requests into them.
        let request = normalize_message_request(&body, requested_model, false)?;
        let responses_body = crate::protocol::anthropic_compat::message_request_to_responses(
            &request,
            requested_model,
        )?;
        let upstream =
            super::responses_http::stream_custom_responses(&state, &headers, responses_body, route)
                .await?;
        return anthropic_message_response(
            upstream,
            requested_model,
            body_stream_requested(&body),
            false,
        )
        .await;
    }
    let resolved = state
        .gateway
        .resolve_native_provider_model(requested_model)?;
    let provider = resolved.provider;
    let upstream_model_id = resolved.upstream_model_id;
    state
        .gateway
        .require_ducx_client(provider, &headers)
        .await?;
    let native_protocol =
        provider.protocol_for_model(upstream_model_id) == ProviderProtocol::AnthropicMessages;
    let native_passthrough = native_protocol && native_passthrough_body(&body);
    let mut request = normalize_message_request(&body, upstream_model_id, native_protocol)?;
    let routing = stable_oneapi_routing(&headers, &body)?;
    let stream_requested = body_stream_requested(&body);
    let client_beta = client_anthropic_beta(&headers);
    if !native_protocol {
        let responses_body = crate::protocol::anthropic_compat::message_request_to_responses(
            &request,
            requested_model,
        )?;
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
    if routing.is_some()
        && !native_passthrough
        && state
            .gateway
            .context_engine()
            .is_some_and(|engine| engine.is_enabled())
    {
        let responses_body =
            crate::protocol::anthropic_compat::native_message_request_to_responses(
                &request,
                &body,
                requested_model,
            )?;
        let mut plan = RequestPlan::provider(
            resolved.catalog_slug.to_owned(),
            provider.id().to_owned(),
            upstream_model_id.to_owned(),
            responses_body,
            routing,
            Some(requested_model.to_owned()),
        )?;
        request.messages.clear();
        request.system = None;
        let mut original_fields = body;
        if let Some(fields) = original_fields.as_object_mut() {
            fields.remove("messages");
        }
        plan.native_messages = Some((request, original_fields));
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
    let mut passthrough_body = native_passthrough.then(|| {
        let mut raw = std::mem::take(&mut body);
        raw["model"] = json!(upstream_model_id);
        raw
    });
    let first = state
        .upstream
        .anthropic_stream_with_web_search_retry(
            provider,
            request,
            hash_key.as_deref(),
            client_beta.as_deref(),
            &mut passthrough_body,
        )
        .await;
    let upstream = match first {
        Ok(upstream) => upstream,
        Err(error @ GatewayError::UpstreamStatus { status, .. })
            if status == StatusCode::PAYLOAD_TOO_LARGE =>
        {
            if let Some(original) = passthrough_body.take() {
                body = original;
            }
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
            let fallback_request =
                normalize_message_request(&fallback_body, upstream_model_id, native_protocol)?;
            let mut passthrough_body = native_passthrough.then(|| {
                let mut raw = fallback_body;
                raw["model"] = json!(upstream_model_id);
                raw
            });
            state
                .upstream
                .anthropic_stream_with_web_search_retry(
                    provider,
                    fallback_request,
                    hash_key.as_deref(),
                    client_beta.as_deref(),
                    &mut passthrough_body,
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

// Native server tools and opaque blocks cannot be represented in the shared
// Responses loop. Keep their original JSON for passthrough; placeholders only
// let the existing transport inspect routing and hosted-tool settings.
fn native_passthrough_body(body: &Value) -> bool {
    let mut passthrough = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| !matches!(kind, "custom" | "function"))
            })
        });
    if let Some(blocks) = body.get("system").and_then(Value::as_array) {
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("text") {
                passthrough = true;
            }
        }
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                for block in blocks {
                    if !matches!(
                        block.get("type").and_then(Value::as_str),
                        Some(
                            "text"
                                | "image"
                                | "thinking"
                                | "redacted_thinking"
                                | "tool_use"
                                | "tool_result"
                        )
                    ) {
                        passthrough = true;
                    }
                }
            }
        }
    }
    passthrough
}

async fn responses_compatible_message(
    state: &AppState,
    headers: &HeaderMap,
    requested_model: &str,
    stream_requested: bool,
    plan: RequestPlan,
) -> Result<Response, GatewayError> {
    let native = plan.native_messages.is_some();
    let upstream = state.gateway.stream(plan, headers).await?;
    anthropic_message_response(upstream, requested_model, stream_requested, native).await
}

async fn anthropic_message_response(
    upstream: crate::protocol::ResponseStream,
    requested_model: &str,
    stream_requested: bool,
    native: bool,
) -> Result<Response, GatewayError> {
    if stream_requested {
        let stream = crate::protocol::anthropic_compat::responses_to_anthropic_stream(
            upstream,
            requested_model.to_owned(),
            native,
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(stream))
            .map_err(|err| GatewayError::Other(err.into()));
    }
    let collected = crate::gateway::collect_response_stream(upstream).await?;
    let message = crate::protocol::anthropic_compat::collected_to_anthropic_message(
        collected,
        requested_model,
    )?;
    Ok(Json(message).into_response())
}

fn body_stream_requested(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool) == Some(true)
}

/// Betas an Anthropic-native client asked for, in header order.
///
/// Claude Code enables 1M context by sending `context-1m-2025-08-07` here, and
/// provider hosts gate some models on that header, so it must reach the
/// upstream instead of being dropped while the gateway rebuilds the request.
fn client_anthropic_beta(headers: &HeaderMap) -> Option<String> {
    let values = headers
        .get_all("anthropic-beta")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(","))
}

pub(super) fn normalize_message_request(
    body: &Value,
    upstream_model: &str,
    native: bool,
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
        .map(|message| normalize_message(message, native))
        .collect::<Result<Vec<_>, GatewayError>>()?;
    let system = match body.get("system") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(vec![ContentBlock::Text { text: text.clone() }]),
        Some(Value::Array(blocks)) => Some(
            blocks
                .iter()
                .map(|block| {
                    if native && block.get("type").and_then(Value::as_str) != Some("text") {
                        Ok(ContentBlock::Text {
                            text: String::new(),
                        })
                    } else {
                        normalize_content_block(block, false)
                    }
                })
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
        temperature: body.get("temperature").and_then(Value::as_f64),
        top_p: body.get("top_p").and_then(Value::as_f64),
        top_k: body.get("top_k").and_then(Value::as_u64),
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

fn normalize_message(message: &Value, native: bool) -> Result<Message, GatewayError> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("message missing role".to_owned()))?
        .to_owned();
    let content = match message.get("content") {
        Some(Value::String(text)) => vec![ContentBlock::Text { text: text.clone() }],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|block| normalize_content_block(block, native))
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

fn normalize_content_block(block: &Value, native: bool) -> Result<ContentBlock, GatewayError> {
    if native
        && !matches!(
            block.get("type").and_then(Value::as_str),
            Some("text" | "image" | "thinking" | "redacted_thinking" | "tool_use" | "tool_result")
        )
    {
        return Ok(ContentBlock::Text {
            text: String::new(),
        });
    }
    ContentBlock::deserialize(block).map_err(|error| {
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
