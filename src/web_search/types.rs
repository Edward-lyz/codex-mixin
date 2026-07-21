use super::*;

pub(super) const CAPABILITY_FILE_VERSION: u64 = 2;
pub(super) const CAPABILITY_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub(super) const PROBE_CONCURRENCY: usize = 4;
pub(super) const NO_EVIDENCE_PROBE_ATTEMPTS: usize = 3;
pub(super) const POSITIVE_CONFIRMATION_ATTEMPTS: usize = 1;
pub(super) const PROBE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const RELEASE_REFERENCE_URL: &str =
    "https://api.github.com/repos/openai/codex/releases/latest";
pub(super) const PROBE_PROMPT: &str = "Use the web_search server tool now to find the latest OpenAI Codex CLI release tag from https://github.com/openai/codex/releases/latest. Never call codex_mixin_probe_noop. Reply with the release tag only.";

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ModelWebSearchCapability {
    pub model: String,
    pub supported: bool,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub probed_at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebSearchProbeSummary {
    pub attempted: usize,
    pub cached: usize,
    pub supported: usize,
    pub unsupported: usize,
    pub failed: usize,
    pub results: Vec<ModelWebSearchCapability>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(super) struct UpstreamIdentity {
    pub(super) provider_preset: String,
    pub(super) upstream_kind: String,
    pub(super) upstream_base_url: String,
    pub(super) upstream_messages_path: String,
    pub(super) web_search_tool_type: String,
}

impl UpstreamIdentity {
    pub(super) fn from_config(config: &GatewayConfig) -> Self {
        Self {
            provider_preset: config.provider_preset.as_str().to_owned(),
            upstream_kind: config.upstream_kind.as_str().to_owned(),
            upstream_base_url: config.upstream_base_url.clone(),
            upstream_messages_path: config.upstream_messages_path.clone(),
            web_search_tool_type: config.web_search_tool_type.clone(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct CapabilitySnapshot {
    pub(super) version: u64,
    pub(super) upstream: UpstreamIdentity,
    pub(super) models: BTreeMap<String, ModelWebSearchCapability>,
}

#[derive(Clone)]
pub struct WebSearchCapabilities {
    pub(super) path: Arc<PathBuf>,
    pub(super) upstream: UpstreamIdentity,
    pub(super) models: Arc<RwLock<BTreeMap<String, ModelWebSearchCapability>>>,
}
