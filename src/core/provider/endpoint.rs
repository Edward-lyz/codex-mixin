use std::borrow::Cow;

use url::Url;

use super::ProviderProtocol;

/// Upstream wire-behavior policy for a resolved `(provider, model)` pair.
///
/// Separated from `ModelCapabilities` (what a model can do) so protocol
/// converters and gateway handlers depend on explicit policy flags rather than
/// string-matching provider IDs, presets, or model suffixes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WirePolicy {
    pub session_affinity_header: Option<String>,
    pub mcp_bridge_names: bool,
    pub unsigned_thinking_replay: bool,
    pub retry_without_opaque_history: bool,
    pub accept_upstream_reported_cost: bool,
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
