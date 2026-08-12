use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::provider::{ProviderAuthHeader, ProviderDefinition, ProviderProtocol};

pub(super) const CAPABILITY_FILE_VERSION: u64 = 2;
pub(super) const PROBE_CONCURRENCY: usize = 4;
pub(super) const PROBE_REQUEST_CONCURRENCY: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Supported,
    Unsupported,
    Indeterminate,
}

impl CapabilityStatus {
    pub(super) fn as_option_bool(self) -> Option<bool> {
        match self {
            Self::Supported => Some(true),
            Self::Unsupported => Some(false),
            Self::Indeterminate => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProtocolCapabilities {
    pub protocol: ProviderProtocol,
    pub api_path: String,
    pub baseline: CapabilityStatus,
    pub image_input: CapabilityStatus,
    pub function_tools: CapabilityStatus,
    pub tool_search: CapabilityStatus,
    pub web_search: CapabilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ModelCapabilities {
    pub model: String,
    pub selected_protocol: Option<ProviderProtocol>,
    pub selected_api_path: Option<String>,
    pub protocols: Vec<ProtocolCapabilities>,
    pub probed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe_error: Option<String>,
}

impl ModelCapabilities {
    pub fn selected(&self) -> Option<&ProtocolCapabilities> {
        let protocol = self.selected_protocol?;
        let api_path = self.selected_api_path.as_deref()?;
        self.protocols
            .iter()
            .find(|candidate| candidate.protocol == protocol && candidate.api_path == api_path)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProviderIdentity {
    pub id: String,
    pub base_url: String,
    pub api_path: String,
    pub configured_protocol: ProviderProtocol,
    pub auth_header: ProviderAuthHeader,
    pub anthropic_version: Option<String>,
    pub anthropic_beta: Option<String>,
    pub custom_header_env_names: Vec<String>,
}

impl ProviderIdentity {
    pub fn from_provider(provider: &ProviderDefinition) -> Self {
        Self {
            id: provider.id.clone(),
            base_url: provider.base_url.clone(),
            api_path: provider.api_path.clone(),
            configured_protocol: provider.protocol,
            auth_header: provider.auth.header,
            anthropic_version: provider.anthropic_version.clone(),
            anthropic_beta: provider.anthropic_beta.clone(),
            custom_header_env_names: provider
                .request_policy
                .custom_headers_from_env
                .keys()
                .cloned()
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(super) struct ProviderCapabilityRecord {
    pub(super) identity: ProviderIdentity,
    pub(super) models: BTreeMap<String, ModelCapabilities>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub(super) struct CapabilityFile {
    pub(super) version: u64,
    pub(super) providers: BTreeMap<String, ProviderCapabilityRecord>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderProbeSummary {
    pub attempted: usize,
    pub supported: usize,
    pub indeterminate: usize,
    pub results: Vec<ModelCapabilities>,
}
