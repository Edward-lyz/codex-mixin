//! Explicit model failover. A configured `FailoverRoute` retries a request
//! that failed before producing any output (429, 5xx, connection or timeout)
//! against each fallback in order. Every switch is logged and announced to the
//! client as a reasoning summary event; fallbacks never cascade.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{LazyLock, Mutex, PoisonError};

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::{GatewayExecutor, RequestPlan, UpstreamTarget};
use crate::config::{
    FailoverActivity, FailoverRoute, FailoverSwitch, OFFICIAL_MODEL_PREFIX, load_failover_activity,
    save_failover_activity,
};
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
    let requested = plan
        .downstream_model
        .as_deref()
        .or_else(|| plan.body.get("model").and_then(Value::as_str));
    if let Some(id) = requested.and_then(|model| {
        model
            .strip_prefix(crate::config::FUSION_MODEL_PREFIX)
            .or_else(|| model.strip_prefix(crate::config::LEGACY_FUSION_MODEL_PREFIX))
    }) {
        return routes.iter().find(|route| route.id.as_deref() == Some(id));
    }
    let source = source_reference(plan)?;
    routes
        .iter()
        .find(|route| route.id.is_none() && route.source == source)
}

pub(super) fn official_route_configured(routes: &[FailoverRoute], body: &Value) -> bool {
    body.get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| {
            routes.iter().any(|route| {
                route.source.starts_with(OFFICIAL_MODEL_PREFIX)
                    && (route.entry_model() == model
                        || (route.id.is_none()
                            && route.source == format!("{OFFICIAL_MODEL_PREFIX}{model}")))
            })
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

static ACTIVITY: LazyLock<Mutex<BTreeMap<String, FailoverActivity>>> = LazyLock::new(|| {
    Mutex::new(load_failover_activity().unwrap_or_else(|error| {
        tracing::warn!(error = %error, "failed to load failover activity");
        BTreeMap::new()
    }))
});

/// Remember which model answered a failover route. The file is rewritten only
/// when the answering model changes or a switch happened.
pub(super) async fn record(source: String, served: Option<String>, switch: Option<FailoverSwitch>) {
    if let Err(error) = tokio::task::spawn_blocking(move || {
        let mut activity = ACTIVITY.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = activity.entry(source).or_default();
        if switch.is_none() && entry.at_ms != 0 && entry.served == served {
            return;
        }
        let now = crate::usage::unix_millis();
        if entry.served != served || entry.at_ms == 0 {
            entry.served = served;
            entry.at_ms = now;
        }
        if let Some(switch) = switch {
            entry.last_switch = Some(switch);
        }
        // Retain a bounded history when routes are renamed or removed.
        if activity.len() > 256
            && let Some(oldest) = activity
                .iter()
                .min_by_key(|(_, entry)| entry.at_ms)
                .map(|(key, _)| key.clone())
        {
            activity.remove(&oldest);
        }
        if let Err(error) = save_failover_activity(&activity) {
            tracing::warn!(error = %error, "failed to save failover activity");
        }
    })
    .await
    {
        tracing::warn!(error = %error, "failover activity writer failed");
    }
}

/// Describe a switch away from the source model.
pub(super) fn switch(
    failed: Vec<String>,
    served: Option<&str>,
    cause: &GatewayError,
) -> FailoverSwitch {
    FailoverSwitch {
        at_ms: crate::usage::unix_millis(),
        failed,
        served: served.map(str::to_owned),
        reason: reason(cause),
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
