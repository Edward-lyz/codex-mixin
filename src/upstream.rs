use std::convert::Infallible;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::stream::BoxStream;
use serde_json::Value;

use crate::error::GatewayError;
use crate::gateway::{RequestPlan, UpstreamExecutor};
use crate::server::AppState;

mod provider;
mod responses;

pub(crate) use provider::{ProviderResponseRequest, stream_provider_response};
pub(crate) use responses::collect_response_stream;

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
    stream_response_with_headers(state, body, &HeaderMap::new()).await
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

pub async fn collect_response(
    state: &AppState,
    mut body: Value,
) -> Result<CollectedResponse, GatewayError> {
    body["stream"] = Value::Bool(true);
    let stream = stream_response(state, body).await?;
    collect_response_stream(stream).await
}
