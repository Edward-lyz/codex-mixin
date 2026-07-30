use std::convert::Infallible;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;

use super::compaction::compact_embedded_tool_images;
use super::{RequestPlan, UpstreamTarget};
use crate::error::GatewayError;
use crate::server::{AppState, stream_official_response};
use crate::sse::{SseDecoder, encode_event, encode_raw_event, event_contains_response_metadata};
use crate::upstream::{ResponseStream, stream_provider_response};

#[derive(Clone, Copy)]
pub(crate) struct UpstreamExecutor<'a> {
    state: &'a AppState,
}

impl<'a> UpstreamExecutor<'a> {
    pub(crate) fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub(crate) async fn stream(
        self,
        plan: RequestPlan,
        headers: &HeaderMap,
    ) -> Result<ResponseStream, GatewayError> {
        self.stream_and_return_body(plan, headers)
            .await
            .map(|(stream, _)| stream)
    }

    pub(crate) async fn stream_and_return_body(
        self,
        mut plan: RequestPlan,
        headers: &HeaderMap,
    ) -> Result<(ResponseStream, serde_json::Value), GatewayError> {
        if matches!(&plan.target, UpstreamTarget::Provider { .. }) {
            let removed_images = compact_embedded_tool_images(&mut plan.body);
            if removed_images > 0 {
                tracing::info!(
                    removed_images,
                    "pruned older embedded tool images from provider request"
                );
            }
        }
        match plan.target {
            UpstreamTarget::Official => {
                let stream = stream_official_response(self.state, headers, &plan.body).await?;
                let stream = match plan.downstream_model {
                    Some(downstream_model) => rewrite_response_model(stream, downstream_model),
                    None => stream,
                };
                Ok((stream, plan.body))
            }
            UpstreamTarget::Provider {
                catalog_slug,
                provider_id,
                upstream_model_id,
                routing,
            } => {
                let provider = self.state.providers.provider(&provider_id).ok_or_else(|| {
                    GatewayError::BadRequest(format!("unknown provider: {provider_id}"))
                })?;
                if provider.uses_ducx_app_server() {
                    let stream = self
                        .state
                        .stream_ducx_response(provider, &upstream_model_id, plan.body.clone())
                        .await?;
                    let stream = match plan.downstream_model {
                        Some(downstream_model) => rewrite_response_model(stream, downstream_model),
                        None => stream,
                    };
                    Ok((stream, plan.body))
                } else {
                    stream_provider_response(
                        self.state,
                        &plan.body,
                        &catalog_slug,
                        &provider_id,
                        &upstream_model_id,
                        routing.as_ref(),
                        plan.downstream_model.as_deref(),
                    )
                    .await
                    .map(|stream| (stream, plan.body))
                }
            }
        }
    }
}

fn rewrite_response_model(mut stream: ResponseStream, downstream_model: String) -> ResponseStream {
    let rewritten = async_stream::stream! {
        let mut decoder = SseDecoder::default();
        while let Some(chunk) = stream.next().await {
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(never) => match never {},
            };
            for event in decoder.push(&bytes) {
                let event_name = event.event.as_deref().unwrap_or("message");
                if !event_contains_response_metadata(event_name) {
                    yield Ok::<Bytes, Infallible>(encode_raw_event(event_name, &event.data));
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(&event.data) {
                    Ok(mut payload) => {
                        if let Some(response) = payload.get_mut("response") {
                            response["model"] =
                                serde_json::Value::String(downstream_model.clone());
                        }
                        yield Ok::<Bytes, Infallible>(encode_event(event_name, &payload)
                            .expect("rewritten official event is serializable"));
                    }
                    Err(_) => yield Ok(encode_raw_event(event_name, &event.data)),
                }
            }
        }
        if !decoder.remaining().is_empty() {
            yield Ok(Bytes::copy_from_slice(decoder.remaining()));
        }
    };
    rewritten.boxed()
}
