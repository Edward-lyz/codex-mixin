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
        match self {
            Self::Custom => "custom",
            Self::BaiduOneApi => "baidu-oneapi",
            Self::OpenRouter => "openrouter",
            Self::DeepSeek => "deepseek",
            Self::OpenCodeGo => OPEN_CODE_GO_PRESET_ID,
            Self::AwsBedrock => AWS_BEDROCK_PRESET_ID,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Custom => {
                "Any OpenAI Responses, Anthropic Messages, or Chat Completions compatible endpoint"
            }
            Self::BaiduOneApi => "Baidu internal OneAPI with managed DUCX authentication",
            Self::OpenRouter => "OpenRouter multi-model router",
            Self::DeepSeek => "DeepSeek official API",
            Self::OpenCodeGo => "OpenCode Go subscription models",
            Self::AwsBedrock => "Amazon Bedrock Mantle with a Bedrock API key",
        }
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

    pub fn create(self, id: impl Into<String>, api_key: impl Into<String>) -> ProviderDefinition {
        let id = id.into();
        let api_key = api_key.into();
        match self {
            Self::Custom => custom_provider(id, api_key),
            Self::BaiduOneApi => baidu_oneapi_provider(id, api_key),
            Self::OpenRouter => openrouter_provider(id, api_key),
            Self::DeepSeek => deepseek_provider(id, api_key),
            Self::OpenCodeGo => open_code_go_provider(id, api_key),
            Self::AwsBedrock => aws_bedrock_provider(id, api_key),
        }
    }
}

pub fn aws_bedrock_provider(
    id: impl Into<String>,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    let cached_models = vec![
        bedrock_model("anthropic.claude-sonnet-5", "Claude Sonnet 5", 1_000_000),
        bedrock_model("anthropic.claude-opus-4-8", "Claude Opus 4.8", 1_000_000),
        bedrock_model("anthropic.claude-haiku-4-5", "Claude Haiku 4.5", 200_000),
    ];
    let selected_models = cached_models.iter().map(|model| model.id.clone()).collect();
    ProviderDefinition {
        id: id.into(),
        display_name: "Amazon Bedrock (Mantle)".to_owned(),
        enabled: true,
        auxiliary_model_upstream: false,
        preset_id: Some(AWS_BEDROCK_PRESET_ID.to_owned()),
        protocol: ProviderProtocol::AnthropicMessages,
        base_url: AWS_BEDROCK_MANTLE_BASE_URL.to_owned(),
        website_url: Some("https://code.claude.com/docs/zh-CN/amazon-bedrock".to_owned()),
        api_path: "/v1/messages".to_owned(),
        model_source: ProviderModelSource::Static,
        auth: ProviderAuthConfig {
            header: ProviderAuthHeader::AuthorizationBearer,
            api_key: api_key.into(),
            aws_sigv4: None,
        },
        anthropic_version: Some("2023-06-01".to_owned()),
        anthropic_beta: None,
        image_generation_path: None,
        quota_url: None,
        quota_username: None,
        quota_workspace_id: None,
        quota_auth_cookie: None,
        quota_currency: Some("USD".to_owned()),
        quota_parser: ProviderQuotaParser::Generic,
        request_policy: ProviderRequestPolicy::default(),
        selected_models,
        new_models: Vec::new(),
        cached_models,
        models_refreshed_at_ms: None,
        models_refresh_error: None,
    }
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
    provider
}

fn bedrock_model(id: &str, display_name: &str, context_window: u64) -> ProviderModel {
    ProviderModel {
        id: id.to_owned(),
        display_name: Some(display_name.to_owned()),
        context_window: Some(context_window),
        supports_image: Some(true),
        supports_thinking: Some(true),
        supports_web_search: Some(false),
        supports_tool_search: Some(false),
        supports_function_tools: Some(true),
        ..ProviderModel::default()
    }
}

pub fn open_code_go_provider(
    id: impl Into<String>,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    // Keep a seed catalog so a new subscription is usable before the first
    // successful /v1/models refresh. The list mirrors CC Switch's OpenCode Go
    // Codex preset; a live refresh may replace it later.
    let cached_models = vec![
        provider_model("glm-5.2", "GLM 5.2", Some(204_800)),
        provider_model("glm-5.1", "GLM 5.1", Some(204_800)),
        provider_model("kimi-k2.7-code", "Kimi K2.7 Code", Some(262_144)),
        provider_model("deepseek-v4-pro", "DeepSeek V4 Pro", None),
        provider_model("deepseek-v4-flash", "DeepSeek V4 Flash", None),
        provider_model("mimo-v2.5-pro", "MiMo V2.5 Pro", Some(1_048_576)),
    ];
    let selected_models = cached_models.iter().map(|model| model.id.clone()).collect();
    ProviderDefinition {
        id: id.into(),
        display_name: "OpenCode Go".to_owned(),
        enabled: true,
        auxiliary_model_upstream: false,
        preset_id: Some(OPEN_CODE_GO_PRESET_ID.to_owned()),
        // OpenCode Go's chat-completions compatibility endpoint rejects image
        // inputs; the native Responses endpoint is required for vision models.
        protocol: ProviderProtocol::OpenAiResponses,
        base_url: "https://opencode.ai/zen/go".to_owned(),
        website_url: None,
        api_path: "/v1/responses".to_owned(),
        model_source: ProviderModelSource::OpenAiCompatible {
            path: "/v1/models".to_owned(),
        },
        auth: ProviderAuthConfig {
            header: ProviderAuthHeader::AuthorizationBearer,
            api_key: api_key.into(),
            aws_sigv4: None,
        },
        anthropic_version: None,
        anthropic_beta: None,
        image_generation_path: None,
        quota_url: None,
        quota_username: None,
        quota_workspace_id: None,
        quota_auth_cookie: None,
        quota_currency: Some("USD".to_owned()),
        quota_parser: ProviderQuotaParser::OpenCodeGo,
        request_policy: ProviderRequestPolicy::default(),
        selected_models,
        new_models: Vec::new(),
        cached_models,
        models_refreshed_at_ms: None,
        models_refresh_error: None,
    }
}

pub fn custom_provider(id: impl Into<String>, api_key: impl Into<String>) -> ProviderDefinition {
    ProviderDefinition {
        id: id.into(),
        display_name: "Custom".to_owned(),
        enabled: true,
        auxiliary_model_upstream: false,
        preset_id: Some("custom".to_owned()),
        // Prefer native Responses. Live probing may replace this when the site
        // only exposes Messages or Chat Completions.
        protocol: ProviderProtocol::OpenAiResponses,
        base_url: String::new(),
        website_url: None,
        api_path: "/v1/responses".to_owned(),
        model_source: ProviderModelSource::OpenAiCompatible {
            path: "/v1/models".to_owned(),
        },
        auth: ProviderAuthConfig {
            header: ProviderAuthHeader::AuthorizationBearer,
            api_key: api_key.into(),
            aws_sigv4: None,
        },
        anthropic_version: None,
        anthropic_beta: None,
        image_generation_path: None,
        quota_url: None,
        quota_username: None,
        quota_workspace_id: None,
        quota_auth_cookie: None,
        quota_currency: None,
        quota_parser: ProviderQuotaParser::Generic,
        request_policy: ProviderRequestPolicy::default(),
        selected_models: Vec::new(),
        new_models: Vec::new(),
        cached_models: Vec::new(),
        models_refreshed_at_ms: None,
        models_refresh_error: None,
    }
}

pub fn baidu_oneapi_provider(
    id: impl Into<String>,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    let mut provider = custom_provider(id, api_key);
    provider.display_name = "Baidu OneAPI".to_owned();
    provider.preset_id = Some("baidu-oneapi".to_owned());
    // Baidu keeps Messages as the default transport; GPT models switch to
    // Responses at request time.
    provider.protocol = ProviderProtocol::AnthropicMessages;
    provider.api_path = "/v1/messages".to_owned();
    provider.anthropic_version = Some("2023-06-01".to_owned());
    provider.base_url = "https://oneapi-comate.baidu-int.com".to_owned();
    provider.model_source = ProviderModelSource::BaiduOneApi;
    provider.image_generation_path = Some("/v1/images/generations".to_owned());
    provider.quota_url =
        Some("https://oneapi-comate.baidu-int.com/openapi/v3/user/quota".to_owned());
    provider.quota_currency = Some("CNY".to_owned());
    provider.quota_parser = ProviderQuotaParser::BaiduOneApi;
    provider.request_policy = ProviderRequestPolicy {
        session_affinity_header: Some("x-hash-key".to_owned()),
        mcp_bridge_for_fable: true,
        custom_headers_from_env: Default::default(),
        baidu_auth_bridge: None,
        ducx_executable: None,
        data_report_executable: None,
        data_report_client_token: None,
        baidu_code_report: false,
    };
    provider
}

pub fn openrouter_provider(
    id: impl Into<String>,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    let mut provider = openai_chat_provider(
        id,
        "OpenRouter",
        "openrouter",
        "https://openrouter.ai/api",
        "/v1/chat/completions",
        "/v1/models",
        api_key,
    );
    provider.quota_url = Some("https://openrouter.ai/api/v1/credits".to_owned());
    provider.quota_currency = Some("USD".to_owned());
    provider.quota_parser = ProviderQuotaParser::OpenRouter;
    provider
}

pub fn deepseek_provider(id: impl Into<String>, api_key: impl Into<String>) -> ProviderDefinition {
    let mut provider = openai_chat_provider(
        id,
        "DeepSeek",
        "deepseek",
        "https://api.deepseek.com",
        "/chat/completions",
        "/models",
        api_key,
    );
    provider.quota_url = Some("https://api.deepseek.com/user/balance".to_owned());
    provider.quota_parser = ProviderQuotaParser::DeepSeek;
    provider
}

fn openai_chat_provider(
    id: impl Into<String>,
    display_name: &str,
    preset_id: &str,
    base_url: &str,
    api_path: &str,
    models_path: &str,
    api_key: impl Into<String>,
) -> ProviderDefinition {
    ProviderDefinition {
        id: id.into(),
        display_name: display_name.to_owned(),
        enabled: true,
        auxiliary_model_upstream: false,
        preset_id: Some(preset_id.to_owned()),
        protocol: ProviderProtocol::OpenAiChat,
        base_url: base_url.to_owned(),
        website_url: None,
        api_path: api_path.to_owned(),
        model_source: ProviderModelSource::OpenAiCompatible {
            path: models_path.to_owned(),
        },
        auth: ProviderAuthConfig {
            header: ProviderAuthHeader::AuthorizationBearer,
            api_key: api_key.into(),
            aws_sigv4: None,
        },
        anthropic_version: None,
        anthropic_beta: None,
        image_generation_path: None,
        quota_url: None,
        quota_username: None,
        quota_workspace_id: None,
        quota_auth_cookie: None,
        quota_currency: None,
        quota_parser: ProviderQuotaParser::Generic,
        request_policy: ProviderRequestPolicy::default(),
        selected_models: Vec::new(),
        new_models: Vec::new(),
        cached_models: Vec::new(),
        models_refreshed_at_ms: None,
        models_refresh_error: None,
    }
}

fn provider_model(id: &str, display_name: &str, context_window: Option<u64>) -> ProviderModel {
    ProviderModel {
        id: id.to_owned(),
        display_name: Some(display_name.to_owned()),
        context_window,
        ..ProviderModel::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn aws_bedrock_preset_uses_mantle_and_static_claude_models() {
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
