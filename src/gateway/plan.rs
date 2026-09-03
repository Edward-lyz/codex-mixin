use serde_json::Value;

use super::ResolvedModelRoute;
use crate::error::GatewayError;
use crate::gateway::UpstreamRouting;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UpstreamTarget {
    Official,
    Provider {
        catalog_slug: String,
        provider_id: String,
        upstream_model_id: String,
        routing: Option<UpstreamRouting>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct RequestPlan {
    pub(crate) target: UpstreamTarget,
    pub(crate) body: Value,
    pub(crate) downstream_model: Option<String>,
}

impl RequestPlan {
    pub(crate) fn from_route(
        route: ResolvedModelRoute,
        body: Value,
        routing: Option<UpstreamRouting>,
        downstream_model: Option<String>,
    ) -> Result<Self, GatewayError> {
        require_streaming(&body)?;
        let target = match route {
            ResolvedModelRoute::Official => UpstreamTarget::Official,
            ResolvedModelRoute::Provider {
                catalog_slug,
                provider_id,
                upstream_model_id,
            } => UpstreamTarget::Provider {
                catalog_slug,
                provider_id,
                upstream_model_id,
                routing,
            },
            ResolvedModelRoute::Fusion { profile_id } => {
                return Err(GatewayError::BadRequest(format!(
                    "fusion profile {profile_id} requires orchestration"
                )));
            }
        };
        Ok(Self {
            target,
            body,
            downstream_model,
        })
    }

    pub(crate) fn official(
        body: Value,
        downstream_model: Option<String>,
    ) -> Result<Self, GatewayError> {
        require_streaming(&body)?;
        Ok(Self {
            target: UpstreamTarget::Official,
            body,
            downstream_model,
        })
    }

    pub(crate) fn provider(
        catalog_slug: String,
        provider_id: String,
        upstream_model_id: String,
        body: Value,
        routing: Option<UpstreamRouting>,
        downstream_model: Option<String>,
    ) -> Result<Self, GatewayError> {
        require_streaming(&body)?;
        Ok(Self {
            target: UpstreamTarget::Provider {
                catalog_slug,
                provider_id,
                upstream_model_id,
                routing,
            },
            body,
            downstream_model,
        })
    }
}

fn require_streaming(body: &Value) -> Result<(), GatewayError> {
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(GatewayError::BadRequest(
            "Codex gateway currently requires stream=true".to_owned(),
        ))
    }
}
