use axum::http::HeaderMap;
use serde_json::Value;

use crate::error::GatewayError;
use crate::protocol::{CollectedResponse, ResponseStream};
use crate::server::AppState;

mod cache_shape;
mod cache_usage;
mod executor;
mod plan;
mod provider;
mod responses;
mod router;

pub(crate) use cache_shape::{
    CacheShape, CacheShapeTracker, PrefixObservation, UpstreamCacheObserver,
    observe_upstream_cache_usage, record_provider_prefix,
};
pub(crate) use cache_usage::{ProviderTokenUsage, TokenUsageAggregator};
pub(crate) use executor::UpstreamExecutor;
pub(crate) use plan::{RequestPlan, UpstreamTarget};
pub(crate) use provider::{ProviderResponseRequest, stream_provider_response};
pub(crate) use responses::collect_response_stream;
pub(crate) use router::{
    AUTO_REVIEW_MODEL_SLUG, ModelRouter, ResolvedModelRoute, is_official_model_slug,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UpstreamRouting {
    pub session_id: String,
    pub hash_key: String,
}

pub(crate) async fn stream_response_with_headers(
    state: &AppState,
    body: Value,
    headers: &HeaderMap,
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
    UpstreamExecutor::new(state).stream(plan, headers).await
}

pub(crate) async fn collect_response_with_headers(
    state: &AppState,
    mut body: Value,
    headers: &HeaderMap,
) -> Result<CollectedResponse, GatewayError> {
    body["stream"] = Value::Bool(true);
    let stream = stream_response_with_headers(state, body, headers).await?;
    collect_response_stream(stream).await
}
