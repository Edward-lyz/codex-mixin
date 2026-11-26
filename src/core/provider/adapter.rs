use std::borrow::Cow;

use url::Url;

use super::endpoint::WirePolicy;
use super::types::{ProviderDefinition, ProviderModel, ProviderModelSource, ProviderProtocol};

/// Provider-specific behavior contract encapsulated behind `ProviderRuntime`.
///
/// Callers outside `provider` interact only with generic `ProviderRuntime` /
/// `EndpointSpec` / `WirePolicy` interfaces; vendor-specific rules (such as
/// Baidu OneAPI routing, display-name formatting, unsigned thinking replay,
/// or cache-metric eligibility) live exclusively inside their adapter.
pub(crate) trait ProviderAdapter: Send + Sync + std::fmt::Debug {
    fn responses_api_path(&self, _def: &ProviderDefinition) -> Option<&'static str> {
        None
    }

    fn models_api_path<'a>(&self, def: &'a ProviderDefinition) -> Option<&'a str> {
        match &def.model_source {
            ProviderModelSource::OpenAiCompatible { path } => Some(path.as_str()),
            ProviderModelSource::BaiduOneApi
            | ProviderModelSource::AwsBedrock
            | ProviderModelSource::Static => None,
        }
    }

    fn protocol_for_model(&self, def: &ProviderDefinition, _model: &str) -> ProviderProtocol {
        def.protocol
    }

    fn api_url_for_model<'a>(
        &self,
        _def: &ProviderDefinition,
        api_url: &'a Url,
        _openai_responses_url: Option<&'a Url>,
        model: &str,
    ) -> Cow<'a, Url> {
        substitute_model_in_url(api_url, model)
    }

    fn wire_policy_for_model(&self, def: &ProviderDefinition, model: &str) -> WirePolicy {
        base_wire_policy(def, model)
    }

    fn format_model_display_name(&self, def: &ProviderDefinition, model: &ProviderModel) -> String {
        format!("{} \u{b7} {}", model.id, def.display_name)
    }

    fn cache_metrics_available(&self, _model_id: &str) -> bool {
        true
    }

    fn uses_native_auth_headers(&self, _def: &ProviderDefinition) -> bool {
        false
    }

    /// Whether this provider needs a managed authentication bridge running.
    fn uses_managed_auth_bridge(&self, _def: &ProviderDefinition) -> bool {
        false
    }

    /// Whether this provider reports usage telemetry through managed hooks.
    fn reports_usage_telemetry(&self, _def: &ProviderDefinition) -> bool {
        false
    }

    /// Configured executable of the managed authentication bridge, if any.
    fn auth_bridge_executable<'a>(
        &self,
        _def: &'a ProviderDefinition,
    ) -> Option<&'a std::path::Path> {
        None
    }
}

#[derive(Debug)]
struct DefaultProviderAdapter;

impl ProviderAdapter for DefaultProviderAdapter {}

static DEFAULT_ADAPTER: DefaultProviderAdapter = DefaultProviderAdapter;

pub(crate) fn adapter_for(def: &ProviderDefinition) -> &'static dyn ProviderAdapter {
    if def.model_source == ProviderModelSource::BaiduOneApi
        || def.preset_id.as_deref() == Some("baidu-oneapi")
        || def.id == "baidu-oneapi"
    {
        &super::baidu::adapter::BAIDU_ONEAPI_ADAPTER
    } else {
        &DEFAULT_ADAPTER
    }
}

/// Whether a provider definition has usage telemetry reporting enabled via its adapter.
pub fn provider_reports_usage_telemetry(def: &ProviderDefinition) -> bool {
    adapter_for(def).reports_usage_telemetry(def)
}

/// Whether a provider definition uses a managed authentication bridge via its adapter.
pub fn provider_uses_managed_auth_bridge(def: &ProviderDefinition) -> bool {
    adapter_for(def).uses_managed_auth_bridge(def)
}

pub(super) fn base_wire_policy(def: &ProviderDefinition, model: &str) -> WirePolicy {
    WirePolicy {
        session_affinity_header: def.request_policy.session_affinity_header.clone(),
        mcp_bridge_names: def.request_policy.mcp_bridge_for_fable
            && model.to_ascii_lowercase().contains("fable"),
        unsigned_thinking_replay: false,
        retry_without_opaque_history: false,
        accept_upstream_reported_cost: super::types::is_managed_pi_provider(def),
    }
}

pub(super) fn substitute_model_in_url<'a>(api_url: &'a Url, model: &str) -> Cow<'a, Url> {
    let raw = api_url.as_str();
    if !raw.contains("{model}") && !raw.contains("%7Bmodel%7D") {
        return Cow::Borrowed(api_url);
    }
    let replaced = raw.replace("{model}", model).replace("%7Bmodel%7D", model);
    Url::parse(&replaced)
        .map(Cow::Owned)
        .unwrap_or_else(|_| Cow::Borrowed(api_url))
}
