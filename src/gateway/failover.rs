//! Explicit model failover. A configured `FailoverRoute` retries a request
//! that failed before producing any output (429, 5xx, connection or timeout)
//! against each fallback in order. Every switch is logged and announced to the
//! client as a reasoning summary event; fallbacks never cascade.

use std::convert::Infallible;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::{GatewayExecutor, RequestPlan, UpstreamTarget};
use crate::config::{FailoverRoute, OFFICIAL_MODEL_PREFIX};
use crate::error::GatewayError;
use crate::protocol::ResponseStream;
use crate::protocol::sse::encode_event;

const STATUS_ITEM_ID: &str = "failover_status";

/// Model reference of the plan in failover-route format.
fn source_reference(plan: &RequestPlan) -> Option<String> {
    match &plan.target {
        UpstreamTarget::Provider { catalog_slug, .. } => Some(catalog_slug.clone()),
        UpstreamTarget::Official => plan
            .body
            .get("model")
            .and_then(Value::as_str)
            .map(|model| format!("{OFFICIAL_MODEL_PREFIX}{model}")),
    }
}

pub(super) fn route_for<'a>(
    routes: &'a [FailoverRoute],
    plan: &RequestPlan,
) -> Option<&'a FailoverRoute> {
    if routes.is_empty() {
        return None;
    }
    let source = source_reference(plan)?;
    routes.iter().find(|route| route.source == source)
}

pub(super) fn official_route_configured(routes: &[FailoverRoute], body: &Value) -> bool {
    body.get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| {
            routes
                .iter()
                .any(|route| route.source == format!("{OFFICIAL_MODEL_PREFIX}{model}"))
        })
}

/// Errors that trigger a switch: rate limits, upstream server errors and
/// transport failures. Everything else (bad request, auth, config) fails fast.
pub(super) fn is_transient(error: &GatewayError) -> bool {
    match error {
        GatewayError::UpstreamStatus { status, .. } => {
            status.as_u16() == 429 || status.is_server_error()
        }
        GatewayError::Http(error) => error.is_connect() || error.is_timeout(),
        _ => false,
    }
}

fn reason(error: &GatewayError) -> String {
    match error {
        GatewayError::UpstreamStatus { status, .. } => status.to_string(),
        GatewayError::Http(error) if error.is_timeout() => "timeout".to_owned(),
        GatewayError::Http(_) => "connection error".to_owned(),
        other => other.to_string(),
    }
}

/// Build the plan for one fallback reference from the untouched request body.
pub(super) fn fallback_plan(
    executor: &GatewayExecutor,
    fallback: &str,
    mut body: Value,
    primary: &RequestPlan,
    headers: &HeaderMap,
) -> Result<RequestPlan, GatewayError> {
    let downstream_model = primary
        .downstream_model
        .clone()
        .or_else(|| body.get("model").and_then(Value::as_str).map(str::to_owned));
    if let Some(model) = fallback.strip_prefix(OFFICIAL_MODEL_PREFIX) {
        body["model"] = Value::String(model.to_owned());
        body["store"] = Value::Bool(false);
        if let Some(fields) = body.as_object_mut() {
            fields.remove("max_output_tokens");
        }
        return RequestPlan::official(body, downstream_model);
    }
    let routing = super::stable_oneapi_routing(headers, &body)?;
    let resolved = executor.resolved_provider_model(fallback)?;
    let native = primary.native_messages.clone().filter(|_| {
        resolved
            .provider
            .protocol_for_model(resolved.upstream_model_id)
            == crate::provider::ProviderProtocol::AnthropicMessages
    });
    body["model"] = Value::String(fallback.to_owned());
    let mut plan = RequestPlan::provider(
        fallback.to_owned(),
        resolved.provider.id().to_owned(),
        resolved.upstream_model_id.to_owned(),
        body,
        routing,
        downstream_model,
    )?;
    plan.native_messages = native;
    Ok(plan)
}

/// Prefix the fallback stream with a visible status line.
pub(super) fn announce(
    stream: ResponseStream,
    source: &str,
    fallback: &str,
    error: &GatewayError,
) -> Result<ResponseStream, GatewayError> {
    let delta = format!(
        "Failover: {source} failed ({}), answering with {fallback}.\n\n",
        reason(error)
    );
    let event = encode_event(
        "response.reasoning_summary_text.delta",
        &json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": STATUS_ITEM_ID,
            "output_index": 0,
            "summary_index": 0,
            "delta": delta,
            "model": fallback,
        }),
    )?;
    let status = futures_util::stream::once(async move { Ok::<Bytes, Infallible>(event) });
    Ok(status.chain(stream).boxed())
}
