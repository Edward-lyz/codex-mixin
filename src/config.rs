use std::env;
use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPreset {
    Custom,
    BaiduOneApi,
    OpenRouter,
    DeepSeek,
}

impl ProviderPreset {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "custom" => Ok(Self::Custom),
            "baidu-oneapi" => Ok(Self::BaiduOneApi),
            "openrouter" => Ok(Self::OpenRouter),
            "deepseek" => Ok(Self::DeepSeek),
            _ => Err(anyhow!("unsupported provider preset: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::BaiduOneApi => "baidu-oneapi",
            Self::OpenRouter => "openrouter",
            Self::DeepSeek => "deepseek",
        }
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Custom => None,
            Self::BaiduOneApi => Some("https://oneapi-comate.baidu-int.com"),
            Self::OpenRouter => Some("https://openrouter.ai/api"),
            Self::DeepSeek => Some("https://api.deepseek.com"),
        }
    }

    pub fn default_quota_url(self, upstream_base_url: &str) -> Option<String> {
        match self {
            Self::BaiduOneApi => Some(format!(
                "{}/openapi/v3/user/quota",
                upstream_base_url.trim_end_matches('/')
            )),
            Self::OpenRouter => Some(format!(
                "{}/v1/credits",
                upstream_base_url.trim_end_matches('/')
            )),
            Self::Custom | Self::DeepSeek => None,
        }
    }

    pub fn default_upstream_kind(self) -> UpstreamKind {
        match self {
            Self::Custom | Self::BaiduOneApi => UpstreamKind::AnthropicMessages,
            Self::OpenRouter | Self::DeepSeek => UpstreamKind::OpenAiChat,
        }
    }

    pub fn default_messages_path(self) -> &'static str {
        match self {
            Self::Custom | Self::BaiduOneApi => "/v1/messages",
            Self::OpenRouter => "/v1/chat/completions",
            Self::DeepSeek => "/chat/completions",
        }
    }

    pub fn default_models_path(self) -> &'static str {
        match self {
            Self::DeepSeek => "/models",
            Self::Custom | Self::BaiduOneApi | Self::OpenRouter => "/v1/models",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamKind {
    AnthropicMessages,
    OpenAiChat,
}

impl UpstreamKind {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "anthropic_messages" | "anthropic-messages" | "anthropic" => {
                Ok(Self::AnthropicMessages)
            }
            "openai_chat" | "openai-chat" | "chat_completions" | "chat-completions" => {
                Ok(Self::OpenAiChat)
            }
            _ => Err(anyhow!("unsupported upstream kind: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic_messages",
            Self::OpenAiChat => "openai_chat",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamAuthHeader {
    AuthorizationBearer,
    XApiKey,
}

impl UpstreamAuthHeader {
    fn from_env_value(value: &str) -> anyhow::Result<Self> {
        match value {
            "authorization" | "bearer" | "authorization-bearer" => Ok(Self::AuthorizationBearer),
            "x-api-key" | "x_api_key" => Ok(Self::XApiKey),
            _ => Err(anyhow!("unsupported upstream auth header: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingMode {
    Off,
    Manual,
    Adaptive,
    Auto,
}

impl ThinkingMode {
    fn from_env_value(value: &str) -> anyhow::Result<Self> {
        match value {
            "off" => Ok(Self::Off),
            "anthropic" | "manual" => Ok(Self::Manual),
            "adaptive" => Ok(Self::Adaptive),
            "auto" => Ok(Self::Auto),
            _ => Err(anyhow!("unsupported CODEX_GATEWAY_THINKING_MODE: {value}")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub bind: SocketAddr,
    pub provider_preset: ProviderPreset,
    pub upstream_kind: UpstreamKind,
    pub upstream_base_url: String,
    pub upstream_messages_path: String,
    pub upstream_models_path: String,
    pub upstream_api_key: String,
    pub official_responses_url: String,
    pub codex_auth_path: PathBuf,
    pub upstream_auth_header: UpstreamAuthHeader,
    pub anthropic_version: String,
    pub anthropic_beta: Option<String>,
    pub gateway_api_key: Option<String>,
    pub accept_codex_oauth: bool,
    pub default_max_tokens: u64,
    pub default_context_window: u64,
    pub request_timeout: Duration,
    pub thinking_mode: ThinkingMode,
    pub enable_web_search_tool: bool,
    pub web_search_tool_type: String,
    pub web_search_max_uses: Option<u64>,
    pub web_search_exclusive: bool,
    pub web_search_omit_system_instructions: bool,
    pub web_search_latest_user_only: bool,
}

impl GatewayConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let stored_config = load_stored_config()?;
        let provider_preset = first_env_value(&["CODEX_GATEWAY_PROVIDER"])
            .or_else(|| {
                stored_config
                    .as_ref()
                    .and_then(|config| config.provider_preset.clone())
            })
            .map(|value| ProviderPreset::parse(&value))
            .transpose()?
            .unwrap_or(ProviderPreset::Custom);
        let bind = env::var("CODEX_GATEWAY_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8787".to_owned())
            .parse()
            .context("invalid CODEX_GATEWAY_BIND")?;
        let upstream_base_url =
            first_env_value(&["CODEX_GATEWAY_UPSTREAM_BASE_URL", "ANTHROPIC_BASE_URL"])
                .or_else(|| {
                    stored_config
                        .as_ref()
                        .and_then(|config| config.upstream_base_url.clone())
                })
                .or_else(|| {
                    provider_preset
                        .default_base_url()
                        .map(std::borrow::ToOwned::to_owned)
                })
            .ok_or_else(|| {
                anyhow!(
                    "set CODEX_GATEWAY_UPSTREAM_BASE_URL, ANTHROPIC_BASE_URL, choose a provider preset, or run login --base-url <url>"
                )
            })?;
        let upstream_kind = first_env_value(&["CODEX_GATEWAY_UPSTREAM_KIND"])
            .or_else(|| {
                stored_config
                    .as_ref()
                    .and_then(|config| config.upstream_kind.clone())
            })
            .map(|value| UpstreamKind::parse(&value))
            .transpose()?
            .unwrap_or_else(|| provider_preset.default_upstream_kind());
        let upstream_api_key = first_env_value(&[
            "CODEX_GATEWAY_UPSTREAM_API_KEY",
            "ANTHROPIC_API_KEY",
        ])
        .or_else(|| {
            stored_config
                .as_ref()
                .and_then(|config| config.upstream_api_key.clone())
        })
        .ok_or_else(|| {
            anyhow!(
                "set CODEX_GATEWAY_UPSTREAM_API_KEY, ANTHROPIC_API_KEY, or run login --key <key>"
            )
        })?;
        let upstream_auth_header = UpstreamAuthHeader::from_env_value(
            &first_env_value(&["CODEX_GATEWAY_UPSTREAM_AUTH_HEADER"])
                .unwrap_or_else(|| "authorization".to_owned()),
        )?;
        let request_timeout_ms = read_u64_env("CODEX_GATEWAY_REQUEST_TIMEOUT_MS", 600_000)?;
        Ok(Self {
            bind,
            provider_preset,
            upstream_kind,
            upstream_base_url: trim_trailing_slash(upstream_base_url),
            upstream_messages_path: first_env_value(&[
                "CODEX_GATEWAY_MESSAGES_PATH",
                "ANTHROPIC_MESSAGES_PATH",
            ])
            .or_else(|| {
                stored_config
                    .as_ref()
                    .and_then(|config| config.upstream_messages_path.clone())
            })
            .unwrap_or_else(|| provider_preset.default_messages_path().to_owned()),
            upstream_models_path: first_env_value(&[
                "CODEX_GATEWAY_MODELS_PATH",
                "ANTHROPIC_MODELS_PATH",
            ])
            .or_else(|| {
                stored_config
                    .as_ref()
                    .and_then(|config| config.upstream_models_path.clone())
            })
            .unwrap_or_else(|| provider_preset.default_models_path().to_owned()),
            upstream_api_key,
            official_responses_url: env::var("CODEX_GATEWAY_OFFICIAL_RESPONSES_URL")
                .unwrap_or_else(|_| "https://chatgpt.com/backend-api/codex/responses".to_owned()),
            codex_auth_path: default_codex_auth_path(),
            upstream_auth_header,
            anthropic_version: first_env_value(&[
                "CODEX_GATEWAY_ANTHROPIC_VERSION",
                "ANTHROPIC_VERSION",
            ])
            .unwrap_or_else(|| "2023-06-01".to_owned()),
            anthropic_beta: first_env_value(&["CODEX_GATEWAY_ANTHROPIC_BETA", "ANTHROPIC_BETA"]),
            gateway_api_key: env::var("CODEX_GATEWAY_KEY")
                .ok()
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    stored_config
                        .as_ref()
                        .and_then(|config| config.gateway_api_key.clone())
                }),
            accept_codex_oauth: read_bool_env("CODEX_GATEWAY_ACCEPT_CODEX_OAUTH", true)?,
            default_max_tokens: read_u64_env("CODEX_GATEWAY_DEFAULT_MAX_TOKENS", 8192)?,
            default_context_window: read_u64_env(
                "CODEX_GATEWAY_DEFAULT_CONTEXT_WINDOW",
                1_000_000,
            )?,
            request_timeout: Duration::from_millis(request_timeout_ms),
            thinking_mode: ThinkingMode::from_env_value(
                &env::var("CODEX_GATEWAY_THINKING_MODE").unwrap_or_else(|_| "auto".to_owned()),
            )?,
            enable_web_search_tool: read_bool_env("CODEX_GATEWAY_ENABLE_WEB_SEARCH_TOOL", false)?,
            web_search_tool_type: env::var("CODEX_GATEWAY_WEB_SEARCH_TOOL_TYPE")
                .unwrap_or_else(|_| "web_search_20250305".to_owned()),
            web_search_max_uses: read_optional_positive_u64_env(
                "CODEX_GATEWAY_WEB_SEARCH_MAX_USES",
                Some(3),
            )?,
            web_search_exclusive: read_bool_env("CODEX_GATEWAY_WEB_SEARCH_EXCLUSIVE", true)?,
            web_search_omit_system_instructions: read_bool_env(
                "CODEX_GATEWAY_WEB_SEARCH_OMIT_SYSTEM",
                true,
            )?,
            web_search_latest_user_only: read_bool_env(
                "CODEX_GATEWAY_WEB_SEARCH_LATEST_USER_ONLY",
                true,
            )?,
        })
    }

    pub fn upstream_messages_url(&self) -> String {
        format!(
            "{}{}",
            self.upstream_base_url,
            ensure_leading_slash(&self.upstream_messages_path)
        )
    }

    pub fn upstream_models_url(&self) -> String {
        format!(
            "{}{}",
            self.upstream_base_url,
            ensure_leading_slash(&self.upstream_models_path)
        )
    }
}

fn default_codex_auth_path() -> PathBuf {
    codex_home_path().join("auth.json")
}

fn codex_home_path() -> PathBuf {
    std::env::var("CODEX_HOME").ok().map_or_else(
        || {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".codex")
        },
        PathBuf::from,
    )
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StoredGatewayConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_messages_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_models_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_username: Option<String>,
}

pub fn stored_config_path() -> PathBuf {
    if let Some(path) = env::var("CODEX_GATEWAY_CONFIG")
        .ok()
        .filter(|path| !path.is_empty())
    {
        return PathBuf::from(path);
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join(".codex-mixin").join("config.json")
}

pub fn load_stored_config() -> anyhow::Result<Option<StoredGatewayConfig>> {
    load_stored_config_from_path(&stored_config_path())
}

pub fn load_stored_config_from_path(
    path: &std::path::Path,
) -> anyhow::Result<Option<StoredGatewayConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let parsed = serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(parsed))
}

pub fn save_stored_config(config: &StoredGatewayConfig) -> anyhow::Result<PathBuf> {
    let path = stored_config_path();
    save_stored_config_to_path(&path, config)?;
    Ok(path)
}

pub fn save_stored_config_to_path(
    path: &std::path::Path,
    config: &StoredGatewayConfig,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        set_private_dir_permissions(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    set_private_file_permissions(&file)?;
    let content = serde_json::to_vec_pretty(config)?;
    file.write_all(&content)
        .with_context(|| format!("write {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn delete_stored_config() -> anyhow::Result<bool> {
    let path = stored_config_path();
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(true)
}

fn set_private_dir_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 700 {}", path.display()))?;
    }
    Ok(())
}

fn set_private_file_permissions(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn trim_trailing_slash(mut value: String) -> String {
    while value.ends_with('/') {
        value.pop();
    }
    value
}

fn ensure_leading_slash(value: &str) -> String {
    if value.starts_with('/') {
        value.to_owned()
    } else {
        format!("/{value}")
    }
}

fn read_u64_env(name: &str, default: u64) -> anyhow::Result<u64> {
    match env::var(name) {
        Ok(value) => value.parse().with_context(|| format!("invalid {name}")),
        Err(_) => Ok(default),
    }
}

fn first_env_value(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.trim().is_empty()))
}

fn read_bool_env(name: &str, default: bool) -> anyhow::Result<bool> {
    match env::var(name) {
        Ok(value) => match value.as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(anyhow!("invalid {name}: {value}")),
        },
        Err(_) => Ok(default),
    }
}

fn read_optional_positive_u64_env(name: &str, default: Option<u64>) -> anyhow::Result<Option<u64>> {
    match env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => {
            let parsed = value
                .parse::<u64>()
                .with_context(|| format!("invalid {name}"))?;
            if parsed == 0 {
                return Err(anyhow!("{name} must be greater than zero"));
            }
            Ok(Some(parsed))
        }
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_loads_stored_gateway_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = StoredGatewayConfig {
            provider_preset: Some("custom".to_owned()),
            upstream_kind: Some("anthropic_messages".to_owned()),
            upstream_base_url: Some("https://example.test".to_owned()),
            upstream_messages_path: Some("/v1/messages".to_owned()),
            upstream_models_path: Some("/v1/models".to_owned()),
            upstream_api_key: Some("secret-key".to_owned()),
            gateway_api_key: Some("local-key".to_owned()),
            quota_url: Some("https://example.test/quota".to_owned()),
            quota_username: Some("quota-user".to_owned()),
        };
        save_stored_config_to_path(&path, &config).unwrap();
        let loaded = load_stored_config_from_path(&path).unwrap().unwrap();
        assert_eq!(
            loaded.upstream_base_url.as_deref(),
            Some("https://example.test")
        );
        assert_eq!(loaded.provider_preset.as_deref(), Some("custom"));
        assert_eq!(loaded.upstream_kind.as_deref(), Some("anthropic_messages"));
        assert_eq!(
            loaded.upstream_messages_path.as_deref(),
            Some("/v1/messages")
        );
        assert_eq!(loaded.upstream_models_path.as_deref(), Some("/v1/models"));
        assert_eq!(loaded.upstream_api_key.as_deref(), Some("secret-key"));
        assert_eq!(loaded.gateway_api_key.as_deref(), Some("local-key"));
        assert_eq!(
            loaded.quota_url.as_deref(),
            Some("https://example.test/quota")
        );
        assert_eq!(loaded.quota_username.as_deref(), Some("quota-user"));
    }
}
