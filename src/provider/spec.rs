//! Static preset knowledge for providers.
//!
//! One [`ProviderSpec`] row per preset is the single source of preset
//! defaults. Creating a provider stamps the row into a self-contained
//! `ProviderDefinition`; runtime code looks the row up again through
//! [`spec_for`] whenever it needs preset behavior or display metadata.

use super::types::{
    AwsSigV4AuthConfig, ProviderAuthConfig, ProviderAuthHeader, ProviderDefinition, ProviderModel,
    ProviderModelSource, ProviderProtocol, ProviderQuotaParser, ProviderRequestPolicy,
};

pub const OPEN_CODE_GO_PRESET_ID: &str = "opencode-go";
pub const AWS_BEDROCK_PRESET_ID: &str = "aws-bedrock";
pub const AWS_BEDROCK_MANTLE_BASE_URL: &str = "https://bedrock-mantle.us-east-1.api.aws/anthropic";
pub const AWS_BEDROCK_DEFAULT_REGION: &str = "us-east-1";
pub const AWS_BEDROCK_MANTLE_SERVICE: &str = "bedrock-mantle";

pub fn aws_bedrock_mantle_base_url(region: &str) -> String {
    format!("https://bedrock-mantle.{region}.api.aws/anthropic")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPreset {
    Custom,
    BaiduOneApi,
    OpenRouter,
    DeepSeek,
    OpenCodeGo,
    AwsBedrock,
}

impl ProviderPreset {
    pub const ALL: [Self; 6] = [
        Self::Custom,
        Self::BaiduOneApi,
        Self::OpenRouter,
        Self::DeepSeek,
        Self::OpenCodeGo,
        Self::AwsBedrock,
    ];

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "custom" => Ok(Self::Custom),
            "baidu-oneapi" => Ok(Self::BaiduOneApi),
            "openrouter" => Ok(Self::OpenRouter),
            "deepseek" => Ok(Self::DeepSeek),
            "opencode-go" | "opencode_go" => Ok(Self::OpenCodeGo),
            "aws-bedrock" | "amazon-bedrock" => Ok(Self::AwsBedrock),
            _ => anyhow::bail!(
                "unsupported provider preset: {value}; available presets: {}",
                Self::available_presets_csv()
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        self.spec().id
    }

    pub fn description(self) -> &'static str {
        self.spec().description
    }

    pub fn available_presets_csv() -> String {
        Self::ALL
            .iter()
            .map(|preset| preset.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn default_id(self) -> &'static str {
        self.as_str()
    }

    pub fn spec(self) -> &'static ProviderSpec {
        SPECS
            .iter()
            .find(|spec| spec.preset == self)
            .expect("every preset has a spec row")
    }

    pub fn create(self, id: impl Into<String>, api_key: impl Into<String>) -> ProviderDefinition {
        self.spec().create(id, api_key)
    }
}

/// Preset knowledge for a stored provider; unknown or missing preset ids fall
/// back to the custom spec.
pub fn spec_for(preset_id: Option<&str>) -> &'static ProviderSpec {
    preset_id
        .and_then(|value| ProviderPreset::parse(value).ok())
        .unwrap_or(ProviderPreset::Custom)
        .spec()
}

/// Model catalog source declared by a preset. Stamped into the runtime
/// `ProviderModelSource` when an instance is created.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelSourceSpec {
    Static,
    OpenAiCompatible(&'static str),
    BaiduOneApi,
    AwsBedrock,
}

impl ModelSourceSpec {
    fn stamp(self) -> ProviderModelSource {
        match self {
            Self::Static => ProviderModelSource::Static,
            Self::OpenAiCompatible(path) => ProviderModelSource::OpenAiCompatible {
                path: path.to_owned(),
            },
            Self::BaiduOneApi => ProviderModelSource::BaiduOneApi,
            Self::AwsBedrock => ProviderModelSource::AwsBedrock,
        }
    }
}

/// Static knowledge for one provider preset.
pub struct ProviderSpec {
    pub preset: ProviderPreset,
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Icon asset id; each UI maps it onto its bundled art.
    pub icon: &'static str,
    pub website_url: Option<&'static str>,
    pub protocol: ProviderProtocol,
    pub base_url: &'static str,
    pub api_path: &'static str,
    pub anthropic_version: Option<&'static str>,
    pub image_generation_path: Option<&'static str>,
    pub model_source: ModelSourceSpec,
    pub quota_url: Option<&'static str>,
    pub quota_currency: Option<&'static str>,
    pub quota_parser: ProviderQuotaParser,
    /// Catalog seeded at creation so a new provider routes before the first
    /// successful model refresh.
    seed_models: fn() -> Vec<ProviderModel>,
    request_policy: fn() -> ProviderRequestPolicy,
}

impl ProviderSpec {
    pub fn create(&self, id: impl Into<String>, api_key: impl Into<String>) -> ProviderDefinition {
        let cached_models = (self.seed_models)();
        let selected_models = cached_models.iter().map(|model| model.id.clone()).collect();
        ProviderDefinition {
            id: id.into(),
            display_name: self.display_name.to_owned(),
            enabled: true,
            auxiliary_model_upstream: false,
            preset_id: Some(self.id.to_owned()),
            protocol: self.protocol,
            base_url: self.base_url.to_owned(),
            website_url: self.website_url.map(str::to_owned),
            api_path: self.api_path.to_owned(),
            model_source: self.model_source.stamp(),
            auth: ProviderAuthConfig {
                header: ProviderAuthHeader::AuthorizationBearer,
                api_key: api_key.into(),
                aws_sigv4: None,
            },
            anthropic_version: self.anthropic_version.map(str::to_owned),
            anthropic_beta: None,
            image_generation_path: self.image_generation_path.map(str::to_owned),
            quota_url: self.quota_url.map(str::to_owned),
            quota_username: None,
            quota_workspace_id: None,
            quota_auth_cookie: None,
            quota_currency: self.quota_currency.map(str::to_owned),
            quota_parser: self.quota_parser,
            request_policy: (self.request_policy)(),
            selected_models,
            new_models: Vec::new(),
            cached_models,
            models_refreshed_at_ms: None,
            models_refresh_error: None,
        }
    }
}

static SPECS: [ProviderSpec; 6] = [
    ProviderSpec {
        preset: ProviderPreset::Custom,
        id: "custom",
        display_name: "Custom",
        description: "Any OpenAI Responses, Anthropic Messages, or Chat Completions compatible endpoint",
        icon: "custom",
        website_url: None,
        // Prefer native Responses. Live probing may replace this when the site
        // only exposes Messages or Chat Completions.
        protocol: ProviderProtocol::OpenAiResponses,
        base_url: "",
        api_path: "/v1/responses",
        anthropic_version: None,
        image_generation_path: None,
        model_source: ModelSourceSpec::OpenAiCompatible("/v1/models"),
        quota_url: None,
        quota_currency: None,
        quota_parser: ProviderQuotaParser::Generic,
        seed_models: Vec::new,
        request_policy: ProviderRequestPolicy::default,
    },
    ProviderSpec {
        preset: ProviderPreset::BaiduOneApi,
        id: "baidu-oneapi",
        display_name: "Baidu OneAPI",
        description: "Baidu internal OneAPI with managed DUCX authentication",
        icon: "baidu",
        website_url: None,
        // Baidu keeps Messages as the default transport; GPT models switch to
        // Responses at request time.
        protocol: ProviderProtocol::AnthropicMessages,
        base_url: "https://oneapi-comate.baidu-int.com",
        api_path: "/v1/messages",
        anthropic_version: Some("2023-06-01"),
        image_generation_path: Some("/v1/images/generations"),
        model_source: ModelSourceSpec::BaiduOneApi,
        quota_url: Some("https://oneapi-comate.baidu-int.com/openapi/v3/user/quota"),
        quota_currency: Some("CNY"),
        quota_parser: ProviderQuotaParser::BaiduOneApi,
        seed_models: Vec::new,
        request_policy: baidu_request_policy,
    },
    ProviderSpec {
        preset: ProviderPreset::OpenRouter,
        id: "openrouter",
        display_name: "OpenRouter",
        description: "OpenRouter multi-model router",
        icon: "openrouter",
        website_url: None,
        protocol: ProviderProtocol::OpenAiChat,
        base_url: "https://openrouter.ai/api",
        api_path: "/v1/chat/completions",
        anthropic_version: None,
        image_generation_path: None,
        model_source: ModelSourceSpec::OpenAiCompatible("/v1/models"),
        quota_url: Some("https://openrouter.ai/api/v1/credits"),
        quota_currency: Some("USD"),
        quota_parser: ProviderQuotaParser::OpenRouter,
        seed_models: Vec::new,
        request_policy: ProviderRequestPolicy::default,
    },
    ProviderSpec {
        preset: ProviderPreset::DeepSeek,
        id: "deepseek",
        display_name: "DeepSeek",
        description: "DeepSeek official API",
        icon: "deepseek",
        website_url: None,
        protocol: ProviderProtocol::OpenAiChat,
        base_url: "https://api.deepseek.com",
        api_path: "/chat/completions",
        anthropic_version: None,
        image_generation_path: None,
        model_source: ModelSourceSpec::OpenAiCompatible("/models"),
        quota_url: Some("https://api.deepseek.com/user/balance"),
        quota_currency: None,
        quota_parser: ProviderQuotaParser::DeepSeek,
        seed_models: Vec::new,
        request_policy: ProviderRequestPolicy::default,
    },
    ProviderSpec {
        preset: ProviderPreset::OpenCodeGo,
        id: OPEN_CODE_GO_PRESET_ID,
        display_name: "OpenCode Go",
        description: "OpenCode Go subscription models",
        icon: "opencode",
        website_url: None,
        // OpenCode Go's chat-completions compatibility endpoint rejects image
        // inputs; the native Responses endpoint is required for vision models.
        protocol: ProviderProtocol::OpenAiResponses,
        base_url: "https://opencode.ai/zen/go",
        api_path: "/v1/responses",
        anthropic_version: None,
        image_generation_path: None,
        model_source: ModelSourceSpec::OpenAiCompatible("/v1/models"),
        quota_url: None,
        quota_currency: Some("USD"),
        quota_parser: ProviderQuotaParser::OpenCodeGo,
        seed_models: open_code_go_seed_models,
        request_policy: ProviderRequestPolicy::default,
    },
    ProviderSpec {
        preset: ProviderPreset::AwsBedrock,
        id: AWS_BEDROCK_PRESET_ID,
        display_name: "Amazon Bedrock (Mantle)",
        description: "Amazon Bedrock Mantle with a Bedrock API key",
        icon: "aws",
        website_url: Some("https://aws.amazon.com/bedrock/"),
        protocol: ProviderProtocol::AnthropicMessages,
        base_url: AWS_BEDROCK_MANTLE_BASE_URL,
        api_path: "/v1/messages",
        anthropic_version: Some("2023-06-01"),
        image_generation_path: None,
        model_source: ModelSourceSpec::Static,
        quota_url: None,
        quota_currency: Some("USD"),
        quota_parser: ProviderQuotaParser::Generic,
        seed_models: bedrock_seed_models,
        request_policy: ProviderRequestPolicy::default,
    },
];

fn baidu_request_policy() -> ProviderRequestPolicy {
    ProviderRequestPolicy {
        session_affinity_header: Some("x-hash-key".to_owned()),
        mcp_bridge_for_fable: true,
        ..ProviderRequestPolicy::default()
    }
}

// Keep a seed catalog so a new subscription is usable before the first
// successful /v1/models refresh. The list mirrors CC Switch's OpenCode Go
// Codex preset; a live refresh may replace it later.
fn open_code_go_seed_models() -> Vec<ProviderModel> {
    vec![
        seed_model("glm-5.2", "GLM 5.2", Some(204_800)),
        seed_model("glm-5.1", "GLM 5.1", Some(204_800)),
        seed_model("kimi-k2.7-code", "Kimi K2.7 Code", Some(262_144)),
        seed_model("deepseek-v4-pro", "DeepSeek V4 Pro", None),
        seed_model("deepseek-v4-flash", "DeepSeek V4 Flash", None),
        seed_model("mimo-v2.5-pro", "MiMo V2.5 Pro", Some(1_048_576)),
    ]
}

fn bedrock_seed_models() -> Vec<ProviderModel> {
    vec![
        bedrock_seed_model("anthropic.claude-sonnet-5", "Claude Sonnet 5", 1_000_000),
        bedrock_seed_model("anthropic.claude-opus-4-8", "Claude Opus 4.8", 1_000_000),
        bedrock_seed_model("anthropic.claude-haiku-4-5", "Claude Haiku 4.5", 200_000),
    ]
}

fn seed_model(id: &str, display_name: &str, context_window: Option<u64>) -> ProviderModel {
    ProviderModel {
        id: id.to_owned(),
        display_name: Some(display_name.to_owned()),
        context_window,
        ..ProviderModel::default()
    }
}

fn bedrock_seed_model(id: &str, display_name: &str, context_window: u64) -> ProviderModel {
    ProviderModel {
        supports_image: Some(true),
        supports_thinking: Some(true),
        supports_web_search: Some(false),
        supports_tool_search: Some(false),
        supports_function_tools: Some(true),
        ..seed_model(id, display_name, Some(context_window))
    }
}

pub fn custom_provider(id: impl Into<String>, api_key: impl Into<String>) -> ProviderDefinition {
    ProviderPreset::Custom.create(id, api_key)
}

pub fn baidu_oneapi_provider(
    id: impl Into<String>,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    ProviderPreset::BaiduOneApi.create(id, api_key)
}

pub fn openrouter_provider(
    id: impl Into<String>,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    ProviderPreset::OpenRouter.create(id, api_key)
}

pub fn deepseek_provider(id: impl Into<String>, api_key: impl Into<String>) -> ProviderDefinition {
    ProviderPreset::DeepSeek.create(id, api_key)
}

pub fn open_code_go_provider(
    id: impl Into<String>,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    ProviderPreset::OpenCodeGo.create(id, api_key)
}

pub fn aws_bedrock_provider(
    id: impl Into<String>,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    ProviderPreset::AwsBedrock.create(id, api_key)
}

pub fn aws_bedrock_aksk_provider(
    id: impl Into<String>,
    access_key_id: impl Into<String>,
    secret_access_key: impl Into<String>,
    session_token: Option<String>,
    region: impl Into<String>,
) -> ProviderDefinition {
    let region = region.into();
    let mut provider = aws_bedrock_provider(id, "");
    provider.display_name = "Amazon Bedrock (AK/SK)".to_owned();
    provider.base_url = aws_bedrock_mantle_base_url(&region);
    provider.auth.aws_sigv4 = Some(AwsSigV4AuthConfig {
        access_key_id: access_key_id.into(),
        secret_access_key: secret_access_key.into(),
        session_token,
        region,
        service: AWS_BEDROCK_MANTLE_SERVICE.to_owned(),
    });
    provider.model_source = ProviderModelSource::AwsBedrock;
    provider
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_table_covers_every_preset_in_declaration_order() {
        assert_eq!(ProviderPreset::ALL.len(), SPECS.len());
        for (preset, spec) in ProviderPreset::ALL.iter().zip(SPECS.iter()) {
            assert_eq!(spec.preset, *preset);
            assert_eq!(spec.id, preset.as_str());
            assert!(!spec.icon.is_empty());
            assert!(!spec.display_name.is_empty());
            assert!(!spec.description.is_empty());
        }
    }

    #[test]
    fn open_code_go_uses_responses_and_models_compatible_paths() {
        let provider = open_code_go_provider("opencode-go", "secret");
        provider.validate().unwrap();
        assert_eq!(provider.protocol, ProviderProtocol::OpenAiResponses);
        assert_eq!(provider.base_url, "https://opencode.ai/zen/go");
        assert_eq!(provider.api_path, "/v1/responses");
        assert_eq!(provider.quota_parser, ProviderQuotaParser::OpenCodeGo);
        assert_eq!(provider.quota_currency.as_deref(), Some("USD"));
        assert_eq!(provider.quota_workspace_id, None);
        assert_eq!(provider.quota_auth_cookie, None);
        assert_eq!(
            provider.model_source,
            ProviderModelSource::OpenAiCompatible {
                path: "/v1/models".to_owned()
            }
        );
        assert_eq!(
            provider.selected_models,
            [
                "glm-5.2",
                "glm-5.1",
                "kimi-k2.7-code",
                "deepseek-v4-pro",
                "deepseek-v4-flash",
                "mimo-v2.5-pro",
            ]
        );
        assert_eq!(provider.cached_models.len(), 6);
    }

    #[test]
    fn deepseek_preset_configures_balance_endpoint() {
        let provider = deepseek_provider("deepseek", "secret");
        assert_eq!(
            provider.quota_url.as_deref(),
            Some("https://api.deepseek.com/user/balance")
        );
        assert_eq!(provider.quota_parser, ProviderQuotaParser::DeepSeek);
    }

    #[test]
    fn aws_bedrock_api_key_preset_uses_static_claude_models() {
        let provider = aws_bedrock_provider("aws-bedrock", "secret");

        provider.validate().unwrap();
        assert_eq!(provider.protocol, ProviderProtocol::AnthropicMessages);
        assert_eq!(provider.base_url, AWS_BEDROCK_MANTLE_BASE_URL);
        assert_eq!(provider.api_path, "/v1/messages");
        assert_eq!(
            provider.auth.header,
            ProviderAuthHeader::AuthorizationBearer
        );
        assert_eq!(provider.model_source, ProviderModelSource::Static);
        assert_eq!(
            provider.selected_models,
            [
                "anthropic.claude-sonnet-5",
                "anthropic.claude-opus-4-8",
                "anthropic.claude-haiku-4-5",
            ]
        );
        assert!(provider.cached_models.iter().all(|model| {
            model.supports_image == Some(true)
                && model.supports_thinking == Some(true)
                && model.supports_function_tools == Some(true)
        }));
    }

    #[test]
    fn aws_bedrock_aksk_preset_uses_sigv4_credentials() {
        let provider = aws_bedrock_aksk_provider(
            "aws-bedrock",
            "AKIDEXAMPLE",
            "secret-example",
            Some("session-example".to_owned()),
            "eu-west-1",
        );

        provider.validate().unwrap();
        assert!(provider.auth.api_key.is_empty());
        let aws = provider.auth.aws_sigv4.unwrap();
        assert_eq!(aws.access_key_id, "AKIDEXAMPLE");
        assert_eq!(aws.secret_access_key, "secret-example");
        assert_eq!(aws.session_token.as_deref(), Some("session-example"));
        assert_eq!(aws.region, "eu-west-1");
        assert_eq!(aws.service, "bedrock-mantle");
        assert_eq!(provider.model_source, ProviderModelSource::AwsBedrock);
        assert_eq!(
            provider.base_url,
            "https://bedrock-mantle.eu-west-1.api.aws/anthropic"
        );
    }

    #[test]
    fn every_non_custom_preset_is_valid_with_required_credentials() {
        for preset in ProviderPreset::ALL
            .into_iter()
            .filter(|preset| *preset != ProviderPreset::Custom)
        {
            let mut provider = preset.create(preset.default_id(), "secret");
            if preset == ProviderPreset::BaiduOneApi {
                provider.quota_username = Some("quota-user".to_owned());
            }
            provider.validate().unwrap();
        }
    }
}
