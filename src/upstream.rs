use std::convert::Infallible;

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde_json::{Value, json};

use crate::convert::responses_to_anthropic_with_web_search;
use crate::error::GatewayError;
use crate::openai_chat::responses_to_openai_chat;
use crate::openai_events::{
    map_anthropic_sse_with_image_routes, map_openai_chat_sse_with_image_routes,
};
use crate::provider::ProviderProtocol;
use crate::server::AppState;
use crate::sse::{SseDecoder, encode_event, encode_raw_event};

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
    stream_response_with_options(state, body, None, None).await
}

pub(crate) async fn stream_response_with_options(
    state: &AppState,
    body: Value,
    routing: Option<&UpstreamRouting>,
    downstream_model: Option<&str>,
) -> Result<ResponseStream, GatewayError> {
    if body.get("stream").and_then(Value::as_bool) != Some(true) {
        return Err(GatewayError::BadRequest(
            "Codex gateway currently requires stream=true".to_owned(),
        ));
    }
    let catalog_slug = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let resolved = state.resolved_provider_model(&catalog_slug)?;
    let provider = resolved.provider.clone();
    let upstream_model_id = resolved.upstream_model_id.to_owned();
    let mut upstream_body = body.clone();
    upstream_body["model"] = Value::String(upstream_model_id.clone());
    let mut downstream_body = body.clone();
    if let Some(model) = downstream_model {
        downstream_body["model"] = Value::String(model.to_owned());
    }
    let downstream_model = downstream_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&catalog_slug)
        .to_owned();
    let stream = match provider.protocol() {
        ProviderProtocol::AnthropicMessages => {
            let mut converted = responses_to_anthropic_with_web_search(
                &upstream_body,
                &state.config,
                state.web_search_enabled_for_custom_request(&body),
                provider.uses_mcp_bridge_names(&upstream_model_id),
            )?;
            if provider.uses_session_affinity()
                && let Some(routing) = routing
            {
                converted.request.metadata = Some(json!({"session_id": routing.session_id}));
            }
            let upstream = state
                .anthropic_stream_with_web_search_retry(
                    &provider,
                    converted.request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .await?;
            map_anthropic_sse_with_image_routes(
                upstream,
                downstream_body,
                converted.tool_names,
                state.custom_image_routes(&provider),
            )
            .boxed()
        }
        ProviderProtocol::OpenAiChat => {
            let converted = responses_to_openai_chat(&upstream_body)?;
            let upstream_request =
                provider.apply_auth(state.client.post(provider.api_url().clone()));
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
                state.custom_image_routes(&provider),
            )
            .boxed()
        }
        ProviderProtocol::OpenAiResponses => {
            let upstream_request =
                provider.apply_auth(state.client.post(provider.api_url().clone()));
            let upstream = provider
                .apply_session_affinity(
                    upstream_request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .json(&upstream_body)
                .send()
                .await
                .map_err(|error| {
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
    Ok(stream)
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
                        match serde_json::from_str::<Value>(&event.data) {
                            Ok(mut payload) => {
                                rewrite_matching_model_fields(
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
                    let error = json!({"message":error.to_string(),"type":"server_error"});
                    yield Ok(encode_event(
                        "response.failed",
                        &json!({
                            "type":"response.failed",
                            "response":{
                                "id":format!("resp_{}", uuid::Uuid::new_v4().simple()),
                                "object":"response",
                                "status":"failed",
                                "model":downstream_model,
                                "error":error,
                                "output":[]
                            },
                            "error":error
                        }),
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

fn rewrite_matching_model_fields(value: &mut Value, upstream_model: &str, downstream_model: &str) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "model" && value.as_str() == Some(upstream_model) {
                    *value = Value::String(downstream_model.to_owned());
                } else {
                    rewrite_matching_model_fields(value, upstream_model, downstream_model);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite_matching_model_fields(value, upstream_model, downstream_model);
            }
        }
        _ => {}
    }
}

pub async fn collect_response(
    state: &AppState,
    body: Value,
) -> Result<CollectedResponse, GatewayError> {
    collect_response_with_routing(state, body, None).await
}

pub(crate) async fn collect_response_with_routing(
    state: &AppState,
    mut body: Value,
    routing: Option<&UpstreamRouting>,
) -> Result<CollectedResponse, GatewayError> {
    body["stream"] = Value::Bool(true);
    let stream = stream_response_with_options(state, body, routing, None).await?;
    collect_response_stream(stream).await
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
