use std::convert::Infallible;

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde_json::{Value, json};

use crate::config::UpstreamKind;
use crate::convert::responses_to_anthropic_with_web_search;
use crate::error::GatewayError;
use crate::openai_chat::responses_to_openai_chat;
use crate::openai_events::{
    map_anthropic_sse_with_image_routes, map_openai_chat_sse_with_image_routes,
};
use crate::server::AppState;
use crate::sse::SseDecoder;

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
    let mut downstream_body = body.clone();
    if let Some(model) = downstream_model {
        downstream_body["model"] = Value::String(model.to_owned());
    }
    let stream = match state.config.upstream_kind {
        UpstreamKind::AnthropicMessages => {
            let mut converted = responses_to_anthropic_with_web_search(
                &body,
                &state.config,
                state.web_search_enabled_for_custom_request(&body),
            )?;
            if let Some(routing) = routing {
                converted.request.metadata = Some(json!({"session_id": routing.session_id}));
            }
            let upstream = state
                .anthropic_stream_with_web_search_retry(
                    converted.request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .await?;
            map_anthropic_sse_with_image_routes(
                upstream,
                downstream_body,
                converted.tool_names,
                state.custom_image_routes(),
            )
            .boxed()
        }
        UpstreamKind::OpenAiChat => {
            let converted = responses_to_openai_chat(&body)?;
            let upstream_request =
                state.apply_upstream_auth(state.client.post(state.config.upstream_messages_url()));
            let upstream = state
                .apply_oneapi_affinity(
                    upstream_request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .header(reqwest::header::ACCEPT, "text/event-stream")
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
            map_openai_chat_sse_with_image_routes(
                upstream.bytes_stream(),
                downstream_body,
                converted.tool_names,
                state.custom_image_routes(),
            )
            .boxed()
        }
    };
    Ok(stream)
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
    let mut stream = stream_response_with_options(state, body, routing, None).await?;
    let mut decoder = SseDecoder::default();
    let mut completed = None;
    let mut terminal_error = None;
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
    let response = completed.ok_or_else(|| {
        GatewayError::Upstream("upstream ended without response.completed".to_owned())
    })?;
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
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
