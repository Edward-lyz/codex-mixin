use std::convert::Infallible;
use std::sync::Arc;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;

use super::router::stable_oneapi_routing;
use super::{RequestPlan, UpstreamTarget};
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::gateway::{
    CacheShape, CacheShapeTracker, PrefixObservation, ProviderResponseRequest,
    record_provider_prefix, stream_provider_response,
};
use crate::images::{
    ImageRouteRegistry, normalize_provider_images_blocking, normalize_provider_images_for_fallback,
};
use crate::protocol::ResponseStream;
use crate::protocol::compaction::TOKEN_PREFIX;
use crate::protocol::sse::{
    SseDecoder, encode_event, encode_raw_event, event_contains_response_metadata,
};
use crate::provider::{ProviderRegistry, ProviderRuntime};
use crate::upstream::UpstreamAccess;
use crate::web_search::WebSearchCapabilities;

struct ProviderStreamPlan {
    downstream_model: Option<String>,
    catalog_slug: String,
    provider_id: String,
    upstream_model_id: String,
    routing: Option<super::UpstreamRouting>,
}

/// Gateway execution component: routing, single-request execution, and cache
/// observation. Owns the components a request needs and never depends on the
/// server or CLI layers.
#[derive(Clone)]
pub(crate) struct GatewayExecutor {
    pub(crate) config: Arc<GatewayConfig>,
    pub(crate) providers: Arc<ProviderRegistry>,
    pub(crate) upstream: Arc<UpstreamAccess>,
    pub(crate) cache_shapes: Arc<CacheShapeTracker>,
    web_search_capabilities: WebSearchCapabilities,
    image_routes: ImageRouteRegistry,
}

/// A completed official upstream send with its prefix observation, ready for
/// the server layer to wrap into an HTTP response.
pub(crate) struct OfficialSendResult {
    pub(crate) response: reqwest::Response,
    pub(crate) observation: Option<PrefixObservation>,
}

impl GatewayExecutor {
    pub(crate) fn new(
        config: Arc<GatewayConfig>,
        providers: Arc<ProviderRegistry>,
        upstream: Arc<UpstreamAccess>,
        cache_shapes: Arc<CacheShapeTracker>,
        web_search_capabilities: WebSearchCapabilities,
        image_routes: ImageRouteRegistry,
    ) -> Self {
        Self {
            config,
            providers,
            upstream,
            cache_shapes,
            web_search_capabilities,
            image_routes,
        }
    }

    pub(crate) async fn resolve_model_route(
        &self,
        model: &str,
    ) -> Result<super::ResolvedModelRoute, GatewayError> {
        let custom_result =
            super::ModelRouter::new(&self.config.fusion_profiles, &self.providers, false)
                .resolve(model);
        if custom_result.is_ok() || !super::is_official_model_slug(model) {
            return custom_result;
        }
        if !model.eq_ignore_ascii_case(super::AUTO_REVIEW_MODEL_SLUG)
            && self
                .config
                .official_selected_models
                .as_ref()
                .is_some_and(|selected| {
                    !selected
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(model))
                })
        {
            return Err(GatewayError::BadRequest(format!(
                "official model is not selected: {model}"
            )));
        }
        if self.config.accept_codex_oauth && self.upstream.official_auth().await.is_ok() {
            return Ok(super::ResolvedModelRoute::Official);
        }
        if model.eq_ignore_ascii_case(super::AUTO_REVIEW_MODEL_SLUG)
            && let Some(resolved) = self
                .providers
                .resolve_available_model(super::AUTO_REVIEW_MODEL_SLUG)
        {
            return Ok(super::ResolvedModelRoute::Provider {
                catalog_slug: resolved.catalog_slug.to_owned(),
                provider_id: resolved.provider.id().to_owned(),
                upstream_model_id: resolved.upstream_model_id.to_owned(),
            });
        }
        custom_result
    }

    pub(crate) fn resolved_provider_model(
        &self,
        catalog_slug: &str,
    ) -> Result<crate::provider::ResolvedProviderModel<'_>, GatewayError> {
        self.providers
            .resolve(catalog_slug)
            .or_else(|| {
                self.providers
                    .resolve_known(catalog_slug)
                    .filter(|resolved| {
                        resolved.provider.definition().enabled
                            && resolved.provider.definition().auxiliary_model_upstream
                            && resolved.model.is_some()
                    })
            })
            .ok_or_else(|| {
                GatewayError::BadRequest(format!("model is not routable: {catalog_slug}"))
            })
    }

    pub(crate) fn resolve_native_provider_model(
        &self,
        model: &str,
    ) -> Result<crate::provider::ResolvedProviderModel<'_>, GatewayError> {
        self.providers
            .resolve_native_model(model)
            .ok_or_else(|| GatewayError::BadRequest(format!("model is not routable: {model}")))
    }

    pub(crate) fn web_search_enabled_for_custom_request(&self, body: &serde_json::Value) -> bool {
        let model = body
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        self.config.enable_web_search_tool && self.web_search_capabilities.supports_model(model)
    }

    pub(crate) fn custom_image_routes(
        &self,
        provider: &ProviderRuntime,
    ) -> Option<ImageRouteRegistry> {
        provider
            .image_generation_url()
            .is_some()
            .then(|| self.image_routes.for_provider(provider.id()))
    }

    pub(crate) fn require_ducx_client(
        &self,
        provider: &ProviderRuntime,
        headers: &HeaderMap,
    ) -> Result<(), GatewayError> {
        if !provider.uses_ducx_loopback() {
            return Ok(());
        }
        self.config
            .gateway_client_keys
            .authenticate(headers)
            .map(|_| ())
            .ok_or(GatewayError::Unauthorized)
    }

    pub(crate) async fn stream(
        &self,
        plan: RequestPlan,
        headers: &HeaderMap,
    ) -> Result<ResponseStream, GatewayError> {
        self.stream_and_return_body(plan, headers)
            .await
            .map(|(stream, _)| stream)
    }

    pub(crate) async fn stream_and_return_body(
        &self,
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
                let stream = self.stream_official(headers, &plan.body).await?;
                let stream = match plan.downstream_model {
                    Some(model) => rewrite_response_model(stream, model),
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

    pub(crate) async fn stream_official(
        &self,
        headers: &HeaderMap,
        body: &serde_json::Value,
    ) -> Result<ResponseStream, GatewayError> {
        let model = body
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("official")
            .to_owned();
        let observation = self.official_prefix_observation(headers, body)?;
        let mut upstream = self
            .upstream
            .send_official_responses(headers, body.clone())
            .await?;
        if upstream.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            let (fallback_body, stats) =
                normalize_provider_images_for_fallback(body.clone()).await?;
            if stats.normalized_images > 0 && stats.saved_bytes > 0 {
                tracing::warn!(
                    normalized_images = stats.normalized_images,
                    saved_image_bytes = stats.saved_bytes,
                    "retrying official responses stream after 413 with aggressively compressed images"
                );
                upstream = self
                    .upstream
                    .send_official_responses(headers, fallback_body)
                    .await?;
            }
        }
        let status = upstream.status();
        if !status.is_success() {
            let body = crate::upstream::body::read_error_text(upstream).await?;
            return Err(GatewayError::UpstreamStatus {
                status,
                message: format!("official responses endpoint returned {status}: {body}"),
            });
        }
        let stream = async_stream::stream! {
            let upstream = crate::gateway::observe_upstream_cache_usage(
                upstream.bytes_stream(),
                observation,
            );
            tokio::pin!(upstream);
            while let Some(chunk) = upstream.next().await {
                match chunk {
                    Ok(bytes) => yield Ok::<Bytes, Infallible>(bytes),
                    Err(error) => {
                        let event = encode_event(
                            "response.failed",
                            &crate::protocol::sse::response_failed_payload(
                                None,
                                Some(&model),
                                error.to_string(),
                                "server_error",
                            ),
                        )
                        .expect("official failure event is serializable");
                        yield Ok(event);
                        break;
                    }
                }
            }
        };
        Ok(stream.boxed())
    }

    /// Send an official request, returning the raw response plus its prefix
    /// observation so the server layer can wrap it with passthrough status.
    pub(crate) async fn send_official(
        &self,
        headers: &HeaderMap,
        body: serde_json::Value,
    ) -> Result<OfficialSendResult, GatewayError> {
        let observation = self.official_prefix_observation(headers, &body)?;
        let mut response = self
            .upstream
            .send_official_responses(headers, body.clone())
            .await?;
        if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            let (fallback_body, stats) = normalize_provider_images_for_fallback(body).await?;
            if stats.normalized_images > 0 && stats.saved_bytes > 0 {
                tracing::warn!(
                    normalized_images = stats.normalized_images,
                    saved_image_bytes = stats.saved_bytes,
                    "retrying official responses request after 413 with aggressively compressed images"
                );
                response = self
                    .upstream
                    .send_official_responses(headers, fallback_body)
                    .await?;
            }
        }
        Ok(OfficialSendResult {
            response,
            observation,
        })
    }

    pub(crate) fn official_prefix_observation(
        &self,
        headers: &HeaderMap,
        body: &serde_json::Value,
    ) -> Result<Option<PrefixObservation>, GatewayError> {
        let model = body
            .get("model")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?;
        let routing = stable_oneapi_routing(headers, body)?;
        Ok(record_provider_prefix(
            &self.cache_shapes,
            "official",
            model,
            model,
            routing.as_ref(),
            CacheShape::from_openai_responses(body),
        ))
    }

    async fn stream_provider(
        &self,
        body: serde_json::Value,
        plan: ProviderStreamPlan,
        headers: &HeaderMap,
    ) -> Result<(ResponseStream, serde_json::Value), GatewayError> {
        let first = stream_provider_response(
            self,
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
            self,
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
