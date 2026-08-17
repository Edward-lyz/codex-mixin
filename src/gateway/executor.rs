use std::convert::Infallible;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;

use super::images::{
    canonicalize_provider_json, normalize_provider_images_blocking,
    normalize_provider_images_for_fallback,
};
use super::{RequestPlan, UpstreamTarget};
use crate::error::GatewayError;
use crate::server::{AppState, stream_official_response};
use crate::sse::{SseDecoder, encode_event, encode_raw_event, event_contains_response_metadata};
use crate::upstream::{ResponseStream, UpstreamRouting, stream_provider_response};

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
            plan.body = prepare_provider_body(plan.body).await?;
        }
        match plan.target {
            UpstreamTarget::Official => {
                self.stream_official(plan.body, plan.downstream_model, headers)
                    .await
            }
            UpstreamTarget::Provider {
                catalog_slug,
                provider_id,
                upstream_model_id,
                routing,
            } => {
                self.stream_provider(
                    plan.body,
                    plan.downstream_model,
                    catalog_slug,
                    provider_id,
                    upstream_model_id,
                    routing,
                )
                .await
            }
        }
    }

    async fn stream_official(
        self,
        body: serde_json::Value,
        downstream_model: Option<String>,
        headers: &HeaderMap,
    ) -> Result<(ResponseStream, serde_json::Value), GatewayError> {
        let stream = stream_official_response(self.state, headers, &body).await?;
        let stream = match downstream_model {
            Some(model) => rewrite_response_model(stream, model),
            None => stream,
        };
        Ok((stream, body))
    }

    async fn stream_provider(
        self,
        body: serde_json::Value,
        downstream_model: Option<String>,
        catalog_slug: String,
        provider_id: String,
        upstream_model_id: String,
        routing: Option<UpstreamRouting>,
    ) -> Result<(ResponseStream, serde_json::Value), GatewayError> {
        let first = stream_provider_response(
            self.state,
            &body,
            &catalog_slug,
            &provider_id,
            &upstream_model_id,
            routing.as_ref(),
            downstream_model.as_deref(),
        )
        .await;
        let Err(error @ GatewayError::UpstreamStatus { status, .. }) = first else {
            return first.map(|stream| (stream, body));
        };
        if status != reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            return Err(error);
        }
        let (fallback_body, stats) = normalize_provider_images_for_fallback(body).await?;
        if stats.normalized_images == 0 || stats.saved_bytes == 0 {
            return Err(error);
        }
        tracing::warn!(
            provider_id,
            upstream_model_id,
            normalized_images = stats.normalized_images,
            saved_image_bytes = stats.saved_bytes,
            "retrying provider request after 413 with aggressively compressed images"
        );
        stream_provider_response(
            self.state,
            &fallback_body,
            &catalog_slug,
            &provider_id,
            &upstream_model_id,
            routing.as_ref(),
            downstream_model.as_deref(),
        )
        .await
        .map(|stream| (stream, fallback_body))
    }
}

async fn prepare_provider_body(
    mut body: serde_json::Value,
) -> Result<serde_json::Value, GatewayError> {
    canonicalize_provider_json(&mut body);
    let (body, image_stats) = normalize_provider_images_blocking(body).await?;
    if image_stats.normalized_images > 0 || image_stats.omitted_tool_images > 0 {
        tracing::info!(
            normalized_images = image_stats.normalized_images,
            omitted_tool_images = image_stats.omitted_tool_images,
            saved_image_bytes = image_stats.saved_bytes,
            "normalized provider image payloads for cache-stable replay"
        );
    }
    Ok(body)
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
