use std::borrow::Cow;

use url::Url;

use super::{ProviderDefinition, ProviderModelSource, ProviderProtocol};

/// Upstream wire-behavior policy for a resolved `(provider, model)` pair.
///
/// Separated from `ModelCapabilities` (what a model can do) so protocol
/// converters and gateway handlers depend on explicit policy flags rather than
/// string-matching provider IDs, presets, or model suffixes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WirePolicy {
    pub session_affinity_header: Option<String>,
    pub mcp_bridge_names: bool,
    pub baidu_oneapi_usage: bool,
    pub unsigned_thinking_replay: bool,
    pub retry_without_opaque_history: bool,
}

impl WirePolicy {
    pub fn uses_session_affinity(&self) -> bool {
        self.session_affinity_header.is_some()
    }
}

/// Single-source-of-truth upstream endpoint specification for a `(provider, model)` pair.
///
/// Shared across Catalog, Capability Probe, Gateway, Failover, and Messages HTTP
/// so every subsystem resolves the exact same protocol, URL, and wire policy.
#[derive(Debug, Clone)]
pub struct EndpointSpec<'a> {
    pub protocol: ProviderProtocol,
    pub url: Cow<'a, Url>,
    pub policy: WirePolicy,
}

impl<'a> EndpointSpec<'a> {
    pub fn api_path(&self) -> &str {
        self.url.path()
    }
}

/// Returns true when the provider is backed by Baidu OneAPI (by model source,
/// preset ID, or legacy `baidu-oneapi` provider ID).
pub fn is_baidu_provider(provider: &ProviderDefinition) -> bool {
    provider.model_source == ProviderModelSource::BaiduOneApi
        || provider.preset_id.as_deref() == Some("baidu-oneapi")
        || provider.id == "baidu-oneapi"
}
