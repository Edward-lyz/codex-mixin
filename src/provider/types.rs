use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use anyhow::ensure;
use reqwest::header::HeaderName;
use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 2;

pub fn is_auto_review_model_id(model_id: &str) -> bool {
    matches!(
        model_id.trim().to_ascii_lowercase().as_str(),
        "codex-auto-review" | "auto" | "auto-baidu-oneapi"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
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
    DuccLoopback,
    DucxLoopback,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProviderAuthConfig {
    pub header: ProviderAuthHeader,
    pub api_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderModelSource {
    OpenAiCompatible { path: String },
    BaiduOneApi,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ducc_executable: Option<PathBuf>,
}

impl ProviderRequestPolicy {
    pub fn effective_baidu_auth_bridge(&self) -> BaiduAuthBridge {
        self.baidu_auth_bridge.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ProviderModel {
    pub id: String,
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
    pub new_models: Vec<String>,
    #[serde(default)]
    pub cached_models: Vec<ProviderModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_refreshed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_refresh_error: Option<String>,
}

impl ProviderDefinition {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_provider_id(&self.id)?;
        ensure!(
            !self.display_name.trim().is_empty(),
            "provider {} display name must not be empty",
            self.id
        );
        validate_base_url(&self.id, &self.base_url)?;
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
            || self.request_policy.ducc_executable.is_some()
        {
            ensure!(
                self.model_source == ProviderModelSource::BaiduOneApi,
                "provider {} can configure a Baidu authentication bridge only for Baidu OneAPI",
                self.id
            );
        }
        if let Some(executable) = &self.request_policy.ducc_executable {
            ensure!(
                executable.is_absolute(),
                "provider {} DUCC executable path must be absolute",
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
                !self.auth.api_key.trim().is_empty(),
                "enabled provider {} must configure an API key",
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
        let cached_ids = cached_ids.into_iter().collect::<HashSet<_>>();
        for model_id in &self.new_models {
            ensure!(
                cached_ids.contains(model_id.as_str()),
                "provider {} marks unavailable model {model_id} as new",
                self.id
            );
        }
        Ok(())
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
        if self.auth.api_key.trim().is_empty() {
            issues.push("api_key_missing".to_owned());
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

    pub fn selects_model(&self, model: &str) -> bool {
        self.selected_models
            .iter()
            .any(|selected| selected == model)
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProviderModelKey {
    pub provider_id: String,
    pub upstream_model_id: String,
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
        id.as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
            && id
                .as_bytes()
                .last()
                .is_some_and(|byte| byte.is_ascii_alphanumeric()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_reports_healthy_degraded_and_disabled_states() {
        let mut provider = crate::provider::open_code_go_provider("provider", "key");
        let readiness = provider.readiness();
        assert_eq!(readiness.status, ProviderReadinessStatus::Healthy);
        assert_eq!(readiness.routable_model_count, 6);

        provider.cached_models.remove(0);
        let readiness = provider.readiness();
        assert_eq!(readiness.status, ProviderReadinessStatus::Degraded);
        assert_eq!(readiness.unavailable_selected_model_count, 1);
        assert!(
            readiness
                .issues
                .contains(&"selected_models_unavailable".to_owned())
        );

        provider.enabled = false;
        assert_eq!(
            provider.readiness().status,
            ProviderReadinessStatus::Disabled
        );
    }

    #[test]
    fn a_refresh_error_degrades_an_otherwise_routable_provider() {
        let mut provider = crate::provider::open_code_go_provider("provider", "key");
        provider.models_refresh_error = Some("upstream unavailable".to_owned());

        let readiness = provider.readiness();

        assert_eq!(readiness.status, ProviderReadinessStatus::Degraded);
        assert!(
            readiness
                .issues
                .contains(&"model_refresh_failed".to_owned())
        );
        assert_eq!(
            readiness.last_model_refresh_error.as_deref(),
            Some("upstream unavailable")
        );
    }

    #[test]
    fn baidu_oneapi_quota_requires_a_username() {
        let mut provider = crate::provider::baidu_oneapi_provider("baidu", "key");
        assert!(
            provider
                .validate()
                .unwrap_err()
                .to_string()
                .contains("requires a quota username")
        );

        provider.quota_username = Some("user@example.com".to_owned());
        provider.validate().unwrap();
    }

    #[test]
    fn opencode_go_quota_requires_both_workspace_id_and_auth_cookie() {
        let mut provider = crate::provider::open_code_go_provider("opencode-go", "key");
        provider.validate().unwrap();

        provider.quota_workspace_id = Some("wrk_123".to_owned());
        assert!(
            provider
                .validate()
                .unwrap_err()
                .to_string()
                .contains("requires both quota workspace ID and auth cookie")
        );

        provider.quota_auth_cookie = Some("cookie".to_owned());
        provider.validate().unwrap();
    }

    #[test]
    fn custom_headers_reject_primary_and_managed_auth_headers() {
        for header in ["authorization", "comate_custom_header"] {
            let mut provider = crate::provider::open_code_go_provider("provider", "key");
            provider.request_policy.custom_headers_from_env =
                BTreeMap::from([(header.to_owned(), "CUSTOM_AUTH".to_owned())]);

            assert!(
                provider
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains(&format!("cannot configure custom header {header}"))
            );
        }
    }

    #[test]
    fn legacy_removed_bridge_deserializes_as_disabled() {
        let policy: ProviderRequestPolicy =
            serde_json::from_str(r#"{"baidu_auth_bridge":"ducx_app_server"}"#).unwrap();
        assert_eq!(
            policy.effective_baidu_auth_bridge(),
            BaiduAuthBridge::Disabled
        );
    }

    #[test]
    fn ducc_loopback_is_limited_to_baidu_oneapi() {
        let mut provider = crate::provider::open_code_go_provider("provider", "key");
        provider.request_policy.baidu_auth_bridge = Some(BaiduAuthBridge::DuccLoopback);
        assert!(
            provider
                .validate()
                .unwrap_err()
                .to_string()
                .contains("Baidu authentication bridge only for Baidu OneAPI")
        );
    }

    #[test]
    fn missing_custom_headers_deserializes_as_empty() {
        let policy: ProviderRequestPolicy =
            serde_json::from_str(r#"{"mcp_bridge_for_fable":true}"#).unwrap();

        assert!(policy.custom_headers_from_env.is_empty());
    }

    #[test]
    fn provider_urls_reject_embedded_credentials_and_query_secrets() {
        for base_url in [
            "https://user:password@example.test",
            "https://example.test?api_key=secret",
            "https://example.test#fragment",
        ] {
            let mut provider = crate::provider::open_code_go_provider("provider", "key");
            provider.base_url = base_url.to_owned();
            assert!(provider.validate().is_err(), "{base_url}");
        }
    }

    #[test]
    fn provider_website_url_round_trips_through_config_json() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://api.example.com".to_owned();
        provider.website_url = Some("https://example.com".to_owned());

        let encoded = serde_json::to_string(&provider).unwrap();
        let decoded: ProviderDefinition = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.website_url.as_deref(), Some("https://example.com"));
    }
}
