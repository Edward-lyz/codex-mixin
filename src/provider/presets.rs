use super::types::{
    ProviderAuthConfig, ProviderAuthHeader, ProviderDefinition, ProviderModel, ProviderModelSource,
    ProviderProtocol, ProviderQuotaParser, ProviderRequestPolicy,
};

pub const OPEN_CODE_GO_PRESET_ID: &str = "opencode-go";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPreset {
    Custom,
    BaiduOneApi,
    OpenRouter,
    DeepSeek,
    OpenCodeGo,
}

impl ProviderPreset {
    pub const ALL: [Self; 5] = [
        Self::Custom,
        Self::BaiduOneApi,
        Self::OpenRouter,
        Self::DeepSeek,
        Self::OpenCodeGo,
    ];

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "custom" => Ok(Self::Custom),
            "baidu-oneapi" => Ok(Self::BaiduOneApi),
            "openrouter" => Ok(Self::OpenRouter),
            "deepseek" => Ok(Self::DeepSeek),
            "opencode-go" | "opencode_go" => Ok(Self::OpenCodeGo),
            _ => anyhow::bail!("unsupported provider preset: {value}"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::BaiduOneApi => "baidu-oneapi",
            Self::OpenRouter => "openrouter",
            Self::DeepSeek => "deepseek",
            Self::OpenCodeGo => OPEN_CODE_GO_PRESET_ID,
        }
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
        }
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
        preset_id: Some(OPEN_CODE_GO_PRESET_ID.to_owned()),
        protocol: ProviderProtocol::OpenAiChat,
        base_url: "https://opencode.ai/zen/go".to_owned(),
        api_path: "/v1/chat/completions".to_owned(),
        model_source: ProviderModelSource::OpenAiCompatible {
            path: "/v1/models".to_owned(),
        },
        auth: ProviderAuthConfig {
            header: ProviderAuthHeader::AuthorizationBearer,
            api_key: api_key.into(),
        },
        anthropic_version: None,
        anthropic_beta: None,
        image_generation_path: None,
        quota_url: None,
        quota_username: None,
        quota_currency: None,
        quota_parser: ProviderQuotaParser::Generic,
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
        preset_id: Some("custom".to_owned()),
        protocol: ProviderProtocol::AnthropicMessages,
        base_url: String::new(),
        api_path: "/v1/messages".to_owned(),
        model_source: ProviderModelSource::OpenAiCompatible {
            path: "/v1/models".to_owned(),
        },
        auth: ProviderAuthConfig {
            header: ProviderAuthHeader::AuthorizationBearer,
            api_key: api_key.into(),
        },
        anthropic_version: Some("2023-06-01".to_owned()),
        anthropic_beta: None,
        image_generation_path: None,
        quota_url: None,
        quota_username: None,
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
    openai_chat_provider(
        id,
        "DeepSeek",
        "deepseek",
        "https://api.deepseek.com",
        "/chat/completions",
        "/models",
        api_key,
    )
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
        preset_id: Some(preset_id.to_owned()),
        protocol: ProviderProtocol::OpenAiChat,
        base_url: base_url.to_owned(),
        api_path: api_path.to_owned(),
        model_source: ProviderModelSource::OpenAiCompatible {
            path: models_path.to_owned(),
        },
        auth: ProviderAuthConfig {
            header: ProviderAuthHeader::AuthorizationBearer,
            api_key: api_key.into(),
        },
        anthropic_version: None,
        anthropic_beta: None,
        image_generation_path: None,
        quota_url: None,
        quota_username: None,
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
    fn open_code_go_uses_chat_and_models_compatible_paths() {
        let provider = open_code_go_provider("opencode-go", "secret");
        provider.validate().unwrap();
        assert_eq!(provider.protocol, ProviderProtocol::OpenAiChat);
        assert_eq!(provider.base_url, "https://opencode.ai/zen/go");
        assert_eq!(provider.api_path, "/v1/chat/completions");
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
