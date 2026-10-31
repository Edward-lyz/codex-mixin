use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::ensure;
use reqwest::header::HeaderName;
use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 2;
pub const MANUAL_MODEL_CONTEXT_WINDOW: u64 = 128_000;
pub(crate) const AUTO_REVIEW_MODEL_ID: &str = "codex-auto-review";

pub fn is_auto_review_model_id(model_id: &str) -> bool {
    matches!(
        model_id.trim().to_ascii_lowercase().as_str(),
        AUTO_REVIEW_MODEL_ID | "auto" | "auto-baidu-oneapi"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
    GoogleGemini,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthHeader {
    AuthorizationBearer,
    XApiKey,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaiduAuthBridge {
    #[default]
    #[serde(alias = "ducx_app_server")]
    Disabled,
    DucxLoopback,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProviderAuthConfig {
    pub header: ProviderAuthHeader,
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_sigv4: Option<AwsSigV4AuthConfig>,
}

impl ProviderAuthConfig {
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty()
            || self
                .aws_sigv4
                .as_ref()
                .is_some_and(AwsSigV4AuthConfig::is_configured)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct AwsSigV4AuthConfig {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    pub region: String,
    pub service: String,
}

impl AwsSigV4AuthConfig {
    pub fn is_configured(&self) -> bool {
        !self.access_key_id.trim().is_empty()
            && !self.secret_access_key.trim().is_empty()
            && !self.region.trim().is_empty()
            && !self.service.trim().is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderModelSource {
    OpenAiCompatible {
        path: String,
    },
    BaiduOneApi,
    AwsBedrock,
    // Read the transient development tag, but keep writes compatible with released CLIs.
    #[serde(alias = "antigravity")]
    Static,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderQuotaParser {
    #[default]
    Generic,
    BaiduOneApi,
    OpenRouter,
    #[serde(rename = "deepseek")]
    DeepSeek,
    #[serde(rename = "opencode_go")]
    OpenCodeGo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProviderRequestPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_affinity_header: Option<String>,
    #[serde(default)]
    pub mcp_bridge_for_fable: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_headers_from_env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baidu_auth_bridge: Option<BaiduAuthBridge>,
    #[serde(
        default,
        alias = "ducc_executable",
        skip_serializing_if = "Option::is_none"
    )]
    pub ducx_executable: Option<PathBuf>,
    /// Dedicated Baidu data-report binary, independent of the auth core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_report_executable: Option<PathBuf>,
    /// Client token captured from a managed data-report warmup and reused by
    /// the Mixin reporter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_report_client_token: Option<String>,
    /// Report Baidu AI code-usage events through Mixin-managed hooks.
    #[serde(default)]
    pub baidu_code_report: bool,
}

impl ProviderRequestPolicy {
    pub fn effective_baidu_auth_bridge(&self) -> BaiduAuthBridge {
        self.baidu_auth_bridge.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProviderModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub manually_added: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ProviderProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_image: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_web_search: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_tool_search: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_function_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_probe_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities_probed_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProviderDefinition {
    pub id: String,
    pub display_name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auxiliary_model_upstream: bool,
    /// Upstream model that answers Codex guardian auto review while this
    /// provider owns the auxiliary upstream. Empty keeps auto review on the
    /// official model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_review_model: Option<String>,
    /// Selected model for Codex requests explicitly marked thread_title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_thread_title_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    pub protocol: ProviderProtocol,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    pub api_path: String,
    pub model_source: ProviderModelSource,
    pub auth: ProviderAuthConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_beta: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_generation_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_auth_cookie: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_currency: Option<String>,
    #[serde(default)]
    pub quota_parser: ProviderQuotaParser,
    #[serde(default)]
    pub request_policy: ProviderRequestPolicy,
    #[serde(default)]
    pub selected_models: Vec<String>,
    #[serde(default)]
    pub model_order: super::ModelOrder,
    #[serde(default)]
    pub new_models: Vec<String>,
    #[serde(default)]
    pub cached_models: Vec<ProviderModel>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_context_overrides: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_refreshed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_refresh_error: Option<String>,
    /// Set when Mixin found this account in a supported local login instead of
    /// the user adding it. Holds a fingerprint of the account identity so a
    /// switched account invalidates the cached model directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_credential: Option<String>,
}

impl ProviderDefinition {
    pub fn apply_model_order(&mut self) {
        self.cached_models
            .sort_by(|left, right| super::compare_model_ids(&left.id, &right.id));
        self.model_order.apply(&mut self.selected_models);
    }

    /// Whether this provider's model catalog declares real per-model
    /// capabilities. Capability and web-search probes are paid completions,
    /// so such providers are never probed.
    pub fn declares_model_capabilities(&self) -> bool {
        self.model_source == ProviderModelSource::BaiduOneApi
            || crate::provider::spec_for(self.preset_id.as_deref()).declares_model_capabilities
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_provider_id(&self.id)?;
        ensure!(
            !self.display_name.trim().is_empty(),
            "provider {} display name must not be empty",
            self.id
        );
        validate_base_url(&self.id, &self.base_url)?;
        for (model, context_window) in &self.model_context_overrides {
            ensure!(
                *context_window > 0,
                "provider {} model {model} context override must be greater than zero",
                self.id
            );
        }
        validate_path(&self.id, "API", &self.api_path)?;
        if let ProviderModelSource::OpenAiCompatible { path } = &self.model_source {
            validate_path(&self.id, "models", path)?;
        }
        if let Some(path) = &self.image_generation_path {
            validate_path(&self.id, "image generation", path)?;
        }
        if let Some(header) = &self.request_policy.session_affinity_header {
            ensure!(
                is_valid_header_name(header),
                "provider {} has invalid session affinity header {header}",
                self.id
            );
        }
        for (header, variable) in &self.request_policy.custom_headers_from_env {
            let parsed_header = HeaderName::from_bytes(header.as_bytes());
            ensure!(
                parsed_header.is_ok(),
                "provider {} has invalid custom header name {header}",
                self.id,
            );
            let header = parsed_header.expect("header checked above");
            ensure!(
                !is_forbidden_custom_header(&header),
                "provider {} cannot configure custom header {header}",
                self.id,
            );
            ensure!(
                is_valid_env_variable_name(variable),
                "provider {} has invalid environment variable name {variable} for custom header {header}",
                self.id,
            );
        }
        if self.request_policy.baidu_auth_bridge.is_some()
            || self.request_policy.ducx_executable.is_some()
            || self.request_policy.data_report_executable.is_some()
            || self.request_policy.data_report_client_token.is_some()
            || self.request_policy.baidu_code_report
        {
            ensure!(
                self.model_source == ProviderModelSource::BaiduOneApi,
                "provider {} can configure a Baidu authentication bridge only for Baidu OneAPI",
                self.id
            );
        }
        if let Some(executable) = &self.request_policy.ducx_executable {
            ensure!(
                executable.is_absolute(),
                "provider {} DUCX executable path must be absolute",
                self.id
            );
            let expected_layout = match self.request_policy.effective_baidu_auth_bridge() {
                BaiduAuthBridge::DucxLoopback => Some((".baidu-cx", "baidu-cx")),
                BaiduAuthBridge::Disabled => None,
            };
            if let Some((root_name, install_name)) = expected_layout {
                let install = executable.parent().and_then(Path::parent);
                let root = install.and_then(Path::parent);
                ensure!(
                    install
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        == Some(install_name)
                        && root
                            .and_then(Path::file_name)
                            .and_then(|name| name.to_str())
                            == Some(root_name),
                    "provider {} {:?} executable path does not match the managed {root_name}/{install_name}/bin layout",
                    self.id,
                    self.request_policy.effective_baidu_auth_bridge()
                );
            }
        }
        if let Some(executable) = &self.request_policy.data_report_executable {
            ensure!(
                executable.is_absolute(),
                "provider {} data-report executable path must be absolute",
                self.id
            );
        }
        if self.quota_parser == ProviderQuotaParser::BaiduOneApi && self.quota_url.is_some() {
            ensure!(
                self.quota_username
                    .as_deref()
                    .is_some_and(|username| !username.trim().is_empty()),
                "provider {} requires a quota username for Baidu OneAPI quota",
                self.id
            );
        }
        if self.quota_parser == ProviderQuotaParser::OpenCodeGo {
            let workspace_id = self
                .quota_workspace_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            let auth_cookie = self
                .quota_auth_cookie
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            ensure!(
                (workspace_id.is_empty() && auth_cookie.is_empty())
                    || (!workspace_id.is_empty() && !auth_cookie.is_empty()),
                "provider {} requires both quota workspace ID and auth cookie for OpenCode Go quota",
                self.id
            );
        }
        if self.enabled {
            ensure!(
                self.has_configured_credentials(),
                "enabled provider {} must configure credentials",
                self.id
            );
        }
        if let Some(aws) = &self.auth.aws_sigv4 {
            ensure!(
                self.auth.api_key.trim().is_empty(),
                "provider {} cannot configure both an API key and AWS SigV4 credentials",
                self.id
            );
            ensure!(
                aws.is_configured(),
                "provider {} must configure AWS access key ID, secret access key, region, and service",
                self.id
            );
            ensure!(
                self.preset_id.as_deref() == Some("aws-bedrock")
                    && self.protocol == ProviderProtocol::AnthropicMessages,
                "provider {} can use AWS SigV4 only with the aws-bedrock Anthropic preset",
                self.id
            );
        }
        ensure_unique_model_ids(&self.id, "selected", &self.selected_models)?;
        ensure_unique_model_ids(&self.id, "new", &self.new_models)?;
        let cached_ids = self
            .cached_models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();
        ensure_unique_model_ids(&self.id, "cached", &cached_ids)?;
        for model in &self.cached_models {
            if let Some(max_output_tokens) = model.max_output_tokens {
                ensure!(
                    max_output_tokens > 0,
                    "provider {} model {} output limit must be greater than zero",
                    self.id,
                    model.id
                );
            }
        }
        let cached_ids = cached_ids.into_iter().collect::<HashSet<_>>();
        for model_id in &self.new_models {
            ensure!(
                cached_ids.contains(model_id.as_str()),
                "provider {} marks unavailable model {model_id} as new",
                self.id
            );
        }
        if let Some(auto_review_model) = self.auto_review_model.as_deref().map(str::trim) {
            ensure!(
                !auto_review_model.is_empty(),
                "provider {} auto review model must not be empty",
                self.id
            );
            ensure!(
                self.selected_models
                    .iter()
                    .any(|selected| selected.eq_ignore_ascii_case(auto_review_model)),
                "provider {} auto review model {auto_review_model} is not one of its selected models",
                self.id
            );
        }
        if let Some(model) = self.codex_thread_title_model.as_deref() {
            ensure!(
                !model.trim().is_empty()
                    && self
                        .selected_models
                        .iter()
                        .any(|selected| selected == model),
                "provider {} thread title model {model} is not one of its selected models",
                self.id
            );
        }
        Ok(())
    }

    /// Drop an auto review model the provider no longer offers, so an upstream
    /// model list change cannot invalidate the stored provider.
    pub fn has_configured_credentials(&self) -> bool {
        self.auth.is_configured()
            || matches!(
                self.preset_id.as_deref(),
                Some("agy" | "agy-business" | "kiro")
            )
    }

    pub fn prune_stale_auto_review_model(&mut self) {
        let Some(auto_review_model) = self.auto_review_model.as_deref().map(str::trim) else {
            return;
        };
        if !self
            .selected_models
            .iter()
            .any(|selected| selected.eq_ignore_ascii_case(auto_review_model))
        {
            self.auto_review_model = None;
        }
    }

    pub fn readiness(&self) -> ProviderReadiness {
        let available_models = self
            .cached_models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<HashSet<_>>();
        let unavailable_selected_model_count = self
            .selected_models
            .iter()
            .filter(|model| !available_models.contains(model.as_str()))
            .count();
        let routable_model_count = self
            .selected_models
            .len()
            .saturating_sub(unavailable_selected_model_count);
        let mut issues = Vec::new();
        if !self.has_configured_credentials() {
            issues.push("credentials_missing".to_owned());
        }
        if routable_model_count == 0 {
            issues.push("no_routable_models".to_owned());
        }
        if unavailable_selected_model_count > 0 {
            issues.push("selected_models_unavailable".to_owned());
        }
        if self.models_refresh_error.is_some() {
            issues.push("model_refresh_failed".to_owned());
        }
        let status = if !self.enabled {
            ProviderReadinessStatus::Disabled
        } else if issues.is_empty() {
            ProviderReadinessStatus::Healthy
        } else {
            ProviderReadinessStatus::Degraded
        };
        ProviderReadiness {
            status,
            routable_model_count,
            selected_model_count: self.selected_models.len(),
            available_model_count: self.cached_models.len(),
            unavailable_selected_model_count,
            models_refreshed_at_ms: self.models_refreshed_at_ms,
            last_model_refresh_error: self.models_refresh_error.clone(),
            issues,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReadinessStatus {
    Healthy,
    Degraded,
    Disabled,
}

impl ProviderReadinessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderReadiness {
    pub status: ProviderReadinessStatus,
    pub routable_model_count: usize,
    pub selected_model_count: usize,
    pub available_model_count: usize,
    pub unavailable_selected_model_count: usize,
    pub models_refreshed_at_ms: Option<u64>,
    pub last_model_refresh_error: Option<String>,
    pub issues: Vec<String>,
}

pub(crate) fn validate_provider_id(id: &str) -> anyhow::Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 64,
        "provider id must contain between 1 and 64 characters"
    );
    ensure!(
        !matches!(id, "official" | "mixin"),
        "provider id {id} is reserved"
    );
    ensure!(
        id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        }),
        "provider id {id} may only contain lowercase letters, numbers, '.', '_' and '-'"
    );
    ensure!(
        id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && id.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric),
        "provider id {id} must start and end with a letter or number"
    );
    Ok(())
}

fn validate_path(provider_id: &str, label: &str, path: &str) -> anyhow::Result<()> {
    ensure!(
        path.starts_with('/') && !path.starts_with("//"),
        "provider {provider_id} {label} path must start with one '/'"
    );
    ensure!(
        !path.contains('?') && !path.contains('#'),
        "provider {provider_id} {label} path must not contain query parameters or fragments"
    );
    Ok(())
}

fn validate_base_url(provider_id: &str, base_url: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(base_url)
        .map_err(|error| anyhow::anyhow!("provider {provider_id} has invalid base URL: {error}"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "provider {provider_id} base URL must use http or https"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "provider {provider_id} base URL must not contain credentials, query parameters, or fragments"
    );
    Ok(())
}

fn is_valid_header_name(header: &str) -> bool {
    !header.is_empty()
        && header.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn is_forbidden_custom_header(name: &HeaderName) -> bool {
    name.as_str().starts_with("proxy-")
        || matches!(
            name.as_str(),
            "authorization"
                | "comate_custom_header"
                | "connection"
                | "content-length"
                | "host"
                | "keep-alive"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "x-api-key"
        )
}

fn is_valid_env_variable_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn ensure_unique_model_ids<T: AsRef<str>>(
    provider_id: &str,
    label: &str,
    model_ids: &[T],
) -> anyhow::Result<()> {
    let mut seen = HashSet::with_capacity(model_ids.len());
    for model_id in model_ids {
        let model_id = model_id.as_ref();
        ensure!(
            !model_id.trim().is_empty(),
            "provider {provider_id} contains an empty {label} model id"
        );
        ensure!(
            seen.insert(model_id),
            "provider {provider_id} contains duplicate {label} model {model_id}"
        );
    }
    Ok(())
}

const fn default_true() -> bool {
    true
}

const fn is_false(value: &bool) -> bool {
    !*value
}
