use std::convert::Infallible;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde_json::{Value, json};

use crate::convert::responses_to_anthropic_with_web_search_and_thinking_kind;
use crate::error::GatewayError;
use crate::gateway::{RequestPlan, UpstreamExecutor};
use crate::model_reasoning::{anthropic_thinking_kind_with_advertised, prepare_upstream_reasoning};
use crate::openai_chat::responses_to_openai_chat_streaming;
use crate::openai_events::{
    map_anthropic_sse_with_image_routes, map_openai_chat_sse_with_image_routes,
};
use crate::provider::ProviderProtocol;
use crate::server::AppState;
use crate::sse::{
    SseDecoder, encode_event, encode_raw_event, event_contains_response_metadata,
    response_failed_payload,
};

pub type ResponseStream = BoxStream<'static, Result<Bytes, Infallible>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpstreamRouting {
    pub session_id: String,
    pub hash_key: String,
}

#[derive(Clone, Debug)]
pub struct CollectedResponse {
    pub response: Value,
    pub output: Vec<Value>,
    pub output_text: String,
    pub usage: Value,
}

pub async fn stream_response(
    state: &AppState,
    body: Value,
) -> Result<ResponseStream, GatewayError> {
    let catalog_slug = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let resolved = state.resolved_provider_model(&catalog_slug)?;
    let plan = RequestPlan::provider(
        catalog_slug,
        resolved.provider.id().to_owned(),
        resolved.upstream_model_id.to_owned(),
        body,
        None,
        None,
    )?;
    UpstreamExecutor::new(state)
        .stream(plan, &HeaderMap::new())
        .await
}

pub async fn collect_response(
    state: &AppState,
    mut body: Value,
) -> Result<CollectedResponse, GatewayError> {
    body["stream"] = Value::Bool(true);
    let stream = stream_response(state, body).await?;
    collect_response_stream(stream).await
}

pub(crate) async fn stream_provider_response(
    state: &AppState,
    body: &Value,
    catalog_slug: &str,
    provider_id: &str,
    upstream_model_id: &str,
    routing: Option<&UpstreamRouting>,
    downstream_model: Option<&str>,
) -> Result<ResponseStream, GatewayError> {
    let provider = state
        .providers
        .provider(provider_id)
        .ok_or_else(|| GatewayError::BadRequest(format!("unknown provider: {provider_id}")))?;
    let upstream_model_id = upstream_model_id.to_owned();
    let downstream_model = downstream_model.unwrap_or(catalog_slug).to_owned();
    let downstream_body = response_metadata_request(body, &downstream_model);
    let web_search_enabled = state.web_search_enabled_for_custom_request(body);
    // DUCC is an Anthropic Messages client. Every selected Baidu model,
    // including GPT/DeepSeek routes, is converted to Messages before the
    // native DUCC request reaches the loopback relay.
    let protocol = if provider.uses_ducc_loopback() {
        ProviderProtocol::AnthropicMessages
    } else {
        provider.protocol_for_model(&upstream_model_id)
    };
    let advertised_thinking = provider.model_supports_thinking(&upstream_model_id);
    let mut upstream_body = body.clone();
    upstream_body["model"] = Value::String(upstream_model_id.clone());
    prepare_upstream_reasoning(&mut upstream_body, advertised_thinking);
    let stream = match protocol {
        ProviderProtocol::AnthropicMessages => {
            let auto_thinking_kind =
                anthropic_thinking_kind_with_advertised(&upstream_model_id, advertised_thinking);
            let converted = responses_to_anthropic_with_web_search_and_thinking_kind(
                &upstream_body,
                &state.config,
                web_search_enabled,
                provider.uses_mcp_bridge_names(&upstream_model_id),
                auto_thinking_kind,
            );
            let mut converted = converted?;
            if provider.uses_session_affinity()
                && let Some(routing) = routing
            {
                converted.request.metadata = Some(json!({"session_id": routing.session_id}));
            }
            let upstream = state
                .anthropic_stream_with_web_search_retry(
                    provider,
                    converted.request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .await?;
            map_anthropic_sse_with_image_routes(
                upstream,
                downstream_body,
                converted.tool_names,
                state.custom_image_routes(provider),
            )
            .boxed()
        }
        ProviderProtocol::OpenAiChat => {
            let converted = responses_to_openai_chat_streaming(&upstream_body)?;
            let upstream_request = provider.apply_auth_for_protocol(
                state
                    .client
                    .post(provider.api_url_for_model(&upstream_model_id).clone()),
                protocol,
            );
            let upstream = provider
                .apply_session_affinity(
                    upstream_request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .json(&converted.request)
                .send()
                .await
                .map_err(|error| {
                    tracing::error!(
                        provider_id = provider.id(),
                        catalog_slug = %catalog_slug,
                        upstream_model_id = %upstream_model_id,
                        error = %crate::error::format_error_chain(&error),
                        "provider chat completions request failed before receiving a response"
                    );
                    GatewayError::Http(error)
                })?;
            let status = upstream.status();
            if !status.is_success() {
                let body = upstream.text().await.unwrap_or_default();
                return Err(GatewayError::Upstream(format!(
                    "provider {} chat completions endpoint returned {status}: {body}",
                    provider.id()
                )));
            }
            map_openai_chat_sse_with_image_routes(
                upstream.bytes_stream(),
                downstream_body,
                converted.tool_names,
                state.custom_image_routes(provider),
            )
            .boxed()
        }
        ProviderProtocol::OpenAiResponses => {
            let upstream_request = provider.apply_auth_for_protocol(
                state
                    .client
                    .post(provider.api_url_for_model(&upstream_model_id).clone()),
                protocol,
            );
            let upstream = provider
                .apply_session_affinity(
                    upstream_request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .json(&upstream_body)
                .send()
                .await;
            let upstream = upstream.map_err(|error| {
                tracing::error!(
                    provider_id = provider.id(),
                    catalog_slug = %catalog_slug,
                    upstream_model_id = %upstream_model_id,
                    error = %crate::error::format_error_chain(&error),
                    "provider responses request failed before receiving a response"
                );
                GatewayError::Http(error)
            })?;
            let status = upstream.status();
            if !status.is_success() {
                let body = upstream.text().await.unwrap_or_default();
                return Err(GatewayError::Upstream(format!(
                    "provider {} responses endpoint returned {status}: {body}",
                    provider.id()
                )));
            }
            map_openai_responses_sse(upstream.bytes_stream(), upstream_model_id, downstream_model)
        }
    };
    let stream = async_stream::stream! {
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            yield chunk;
        }
    };
    Ok(stream.boxed())
}

fn response_metadata_request(body: &Value, downstream_model: &str) -> Value {
    const RESPONSE_FIELDS: &[&str] = &[
        "instructions",
        "max_output_tokens",
        "parallel_tool_calls",
        "previous_response_id",
        "reasoning",
        "store",
        "temperature",
        "text",
        "tool_choice",
        "tools",
        "top_p",
        "truncation",
        "user",
        "metadata",
    ];
    let mut request = serde_json::Map::with_capacity(RESPONSE_FIELDS.len() + 1);
    request.insert(
        "model".to_owned(),
        Value::String(downstream_model.to_owned()),
    );
    for &field in RESPONSE_FIELDS {
        if let Some(value) = body.get(field) {
            request.insert(field.to_owned(), value.clone());
        }
    }
    Value::Object(request)
}

fn map_openai_responses_sse<S>(
    upstream: S,
    upstream_model: String,
    downstream_model: String,
) -> ResponseStream
where
    S: futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::stream! {
        let mut decoder = SseDecoder::default();
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(bytes) => {
                    for event in decoder.push(&bytes) {
                        let event_name = event.event.as_deref().unwrap_or("message");
                        if event.data == "[DONE]" {
                            yield Ok(encode_raw_event(event_name, &event.data));
                            continue;
                        }
                        if !event_contains_response_metadata(event_name) {
                            yield Ok(encode_raw_event(event_name, &event.data));
                            continue;
                        }
                        match serde_json::from_str::<Value>(&event.data) {
                            Ok(mut payload) => {
                                rewrite_response_model_field(
                                    &mut payload,
                                    &upstream_model,
                                    &downstream_model,
                                );
                                yield Ok(encode_event(event_name, &payload)
                                    .expect("rewritten responses event is serializable"));
                            }
                            Err(_) => yield Ok(encode_raw_event(event_name, &event.data)),
                        }
                    }
                }
                Err(error) => {
                    yield Ok(encode_event(
                        "response.failed",
                        &response_failed_payload(
                            None,
                            Some(&downstream_model),
                            error.to_string(),
                            "server_error",
                        ),
                    ).expect("responses transport error is serializable"));
                    return;
                }
            }
        }
        if !decoder.remaining().is_empty() {
            yield Ok(Bytes::copy_from_slice(decoder.remaining()));
        }
    }
    .boxed()
}

fn rewrite_response_model_field(payload: &mut Value, upstream_model: &str, downstream_model: &str) {
    if let Some(model) = payload
        .get_mut("response")
        .and_then(|response| response.get_mut("model"))
        && model.as_str() == Some(upstream_model)
    {
        *model = Value::String(downstream_model.to_owned());
    }
}

pub(crate) async fn collect_response_stream(
    mut stream: ResponseStream,
) -> Result<CollectedResponse, GatewayError> {
    let mut decoder = SseDecoder::default();
    let mut completed = None;
    let mut terminal_error = None;
    let mut observed_output = Vec::new();
    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(never) => match never {},
        };
        for event in decoder.push(&bytes) {
            match event.event.as_deref() {
                Some("response.completed") => {
                    let mut payload: Value = serde_json::from_str(&event.data)?;
                    completed = payload.get_mut("response").map(Value::take);
                }
                Some("response.output_item.done") => {
                    let mut payload: Value = serde_json::from_str(&event.data)?;
                    if let Some(item) = payload.get_mut("item").map(Value::take) {
                        observed_output.push(item);
                    }
                }
                Some("response.failed" | "response.incomplete") => {
                    let payload: Value = serde_json::from_str(&event.data)?;
                    terminal_error = Some(
                        payload
                            .pointer("/error/message")
                            .or_else(|| payload.pointer("/response/error/message"))
                            .and_then(Value::as_str)
                            .unwrap_or("upstream response did not complete")
                            .to_owned(),
                    );
                }
                _ => {}
            }
        }
    }
    if let Some(message) = terminal_error {
        return Err(GatewayError::Upstream(message));
    }
    let mut response = completed.ok_or_else(|| {
        GatewayError::Upstream("upstream ended without response.completed".to_owned())
    })?;
    let mut output = response
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if output.is_empty() && !observed_output.is_empty() {
        output = observed_output;
        response["output"] = Value::Array(output.clone());
    }
    let output_text = collect_output_text(&output);
    let usage = response.get("usage").cloned().unwrap_or(Value::Null);
    Ok(CollectedResponse {
        response,
        output,
        output_text,
        usage,
    })
}

fn collect_output_text(output: &[Value]) -> String {
    output
        .iter()
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("output_text" | "text")
            )
            .then(|| part.get("text").and_then(Value::as_str))
            .flatten()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_metadata_does_not_copy_input_history() {
        let body = json!({
            "model": "upstream",
            "instructions": "system",
            "input": [{"role": "user", "content": "large history"}],
            "tools": [{"type": "function", "name": "lookup"}],
            "stream": true
        });

        let metadata = response_metadata_request(&body, "catalog");

        assert_eq!(metadata["model"], "catalog");
        assert_eq!(metadata["instructions"], "system");
        assert_eq!(metadata["tools"], body["tools"]);
        assert!(metadata.get("input").is_none());
        assert!(metadata.get("stream").is_none());
    }

    #[test]
    fn model_rewrite_only_touches_the_response_metadata() {
        let mut payload = json!({
            "type": "response.completed",
            "response": {
                "model": "upstream",
                "output": [{"model": "upstream"}]
            },
            "model": "upstream"
        });

        rewrite_response_model_field(&mut payload, "upstream", "catalog");

        assert_eq!(payload["response"]["model"], "catalog");
        assert_eq!(payload["response"]["output"][0]["model"], "upstream");
        assert_eq!(payload["model"], "upstream");
    }
}
