use std::convert::Infallible;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;

use super::{RequestPlan, UpstreamTarget};
use crate::error::GatewayError;
use crate::gateway::{
    ProviderResponseRequest, ResponseStream, UpstreamRouting, stream_provider_response,
};
use crate::images::{normalize_provider_images_blocking, normalize_provider_images_for_fallback};
use crate::protocol::compaction::TOKEN_PREFIX;
use crate::protocol::sse::{
    SseDecoder, encode_event, encode_raw_event, event_contains_response_metadata,
};
use crate::server::{AppState, stream_official_response};

struct ProviderStreamPlan {
    downstream_model: Option<String>,
    catalog_slug: String,
    provider_id: String,
    upstream_model_id: String,
    routing: Option<UpstreamRouting>,
}

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
        } else {
            let (body, image_stats) = normalize_provider_images_blocking(plan.body).await?;
            if image_stats.normalized_images > 0 || image_stats.omitted_tool_images > 0 {
                tracing::info!(
                    normalized_images = image_stats.normalized_images,
                    omitted_tool_images = image_stats.omitted_tool_images,
                    saved_image_bytes = image_stats.saved_bytes,
                    "normalized official image payloads for cache-stable replay"
                );
            }
            plan.body = body;
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
                    ProviderStreamPlan {
                        downstream_model: plan.downstream_model,
                        catalog_slug,
                        provider_id,
                        upstream_model_id,
                        routing,
                    },
                    headers,
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
        plan: ProviderStreamPlan,
        headers: &HeaderMap,
    ) -> Result<(ResponseStream, serde_json::Value), GatewayError> {
        let first = stream_provider_response(
            self.state,
            ProviderResponseRequest {
                body: &body,
                catalog_slug: &plan.catalog_slug,
                provider_id: &plan.provider_id,
                upstream_model_id: &plan.upstream_model_id,
                routing: plan.routing.as_ref(),
                downstream_model: plan.downstream_model.as_deref(),
                headers,
            },
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
            provider_id = %plan.provider_id,
            upstream_model_id = %plan.upstream_model_id,
            normalized_images = stats.normalized_images,
            saved_image_bytes = stats.saved_bytes,
            "retrying provider request after 413 with aggressively compressed images"
        );
        stream_provider_response(
            self.state,
            ProviderResponseRequest {
                body: &fallback_body,
                catalog_slug: &plan.catalog_slug,
                provider_id: &plan.provider_id,
                upstream_model_id: &plan.upstream_model_id,
                routing: plan.routing.as_ref(),
                downstream_model: plan.downstream_model.as_deref(),
                headers,
            },
        )
        .await
        .map(|stream| (stream, fallback_body))
    }
}

async fn prepare_provider_body(
    mut body: serde_json::Value,
) -> Result<serde_json::Value, GatewayError> {
    // Provider converters need only the authenticated token. Response item metadata is
    // meaningful to Codex but must not affect a custom provider continuation.
    if let Some(items) = body
        .get_mut("input")
        .and_then(serde_json::Value::as_array_mut)
    {
        for item in items {
            if item.get("type").and_then(serde_json::Value::as_str) == Some("compaction")
                && item
                    .get("encrypted_content")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|token| token.starts_with(TOKEN_PREFIX))
                && let Some(item) = item.as_object_mut()
            {
                item.remove("id");
                item.remove("created_by");
            }
        }
    }
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
