use super::auth::{check_gateway_auth, stable_oneapi_routing};
use super::*;
use crate::anthropic::{ContentBlock, Message, MessageRequest};
use crate::provider::ProviderProtocol;
use async_stream::stream;

pub(super) async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?;
    let resolved = state.resolve_native_provider_model(requested_model)?;
    let provider = resolved.provider;
    let upstream_model_id = resolved.upstream_model_id;
    if provider.protocol_for_model(upstream_model_id) != ProviderProtocol::AnthropicMessages {
        return Err(GatewayError::BadRequest(format!(
            "provider {} does not expose model {upstream_model_id} over Anthropic Messages",
            provider.id()
        )));
    }
    let request = normalize_message_request(&body, upstream_model_id)?;
    let hash_key = stable_oneapi_routing(&headers, &body)?.map(|routing| routing.hash_key);
    let stream_requested = body_stream_requested(&body);
    let upstream = state
        .anthropic_stream_with_web_search_retry(provider, request, hash_key.as_deref())
        .await?;
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
