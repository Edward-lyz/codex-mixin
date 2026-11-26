use std::borrow::Cow;
use std::path::Path;

use url::Url;

use super::super::adapter::{ProviderAdapter, base_wire_policy, substitute_model_in_url};
use super::super::endpoint::WirePolicy;
use super::super::types::{
    BaiduAuthBridge, ProviderDefinition, ProviderModel, ProviderModelSource, ProviderProtocol,
};

/// Model families whose cache counters Baidu OneAPI does not report.
const BAIDU_CLAUDE_FAMILY_NAMES: &[&str] = &["claude", "fable", "opus", "sonet", "sonnet", "haiku"];

#[derive(Debug)]
pub(crate) struct BaiduOneApiAdapter;

impl ProviderAdapter for BaiduOneApiAdapter {
    fn responses_api_path(&self, def: &ProviderDefinition) -> Option<&'static str> {
        (def.model_source == ProviderModelSource::BaiduOneApi).then_some("/v1/responses")
    }

    fn models_api_path<'a>(&self, def: &'a ProviderDefinition) -> Option<&'a str> {
        match &def.model_source {
            ProviderModelSource::BaiduOneApi => Some("/openapi/v2/available_models"),
            ProviderModelSource::OpenAiCompatible { path } => Some(path.as_str()),
            ProviderModelSource::AwsBedrock | ProviderModelSource::Static => None,
        }
    }

    fn protocol_for_model(&self, def: &ProviderDefinition, model: &str) -> ProviderProtocol {
        if def.model_source == ProviderModelSource::BaiduOneApi
            && model.trim().to_ascii_lowercase().starts_with("gpt-")
        {
            ProviderProtocol::OpenAiResponses
        } else {
            def.protocol
        }
    }

    fn api_url_for_model<'a>(
        &self,
        def: &ProviderDefinition,
        api_url: &'a Url,
        openai_responses_url: Option<&'a Url>,
        model: &str,
    ) -> Cow<'a, Url> {
        if self.protocol_for_model(def, model) == ProviderProtocol::OpenAiResponses
            && let Some(url) = openai_responses_url
        {
            return Cow::Borrowed(url);
        }
        substitute_model_in_url(api_url, model)
    }

    fn wire_policy_for_model(&self, def: &ProviderDefinition, model: &str) -> WirePolicy {
        let mut policy = base_wire_policy(def, model);
        policy.unsigned_thinking_replay = true;
        policy.retry_without_opaque_history = true;
        policy
    }

    fn format_model_display_name(
        &self,
        _def: &ProviderDefinition,
        model: &ProviderModel,
    ) -> String {
        let base = model
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&model.id);
        let gpt = base
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("gpt-"));
        let mut name = String::new();
        for (index, word) in base.split('-').enumerate() {
            if index > 0 {
                name.push(if gpt && index == 1 { '-' } else { ' ' });
            }
            match word.to_ascii_lowercase().as_str() {
                "gpt" => name.push_str("GPT"),
                "deepseek" => name.push_str("DeepSeek"),
                _ => {
                    let mut chars = word.chars();
                    if let Some(first) = chars.next() {
                        name.extend(first.to_uppercase());
                        name.extend(chars);
                    }
                }
            }
        }
        if let Some(ratio) = model
            .ratio
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            name.push_str(&format!(" ({ratio})"));
        }
        name.push_str(" \u{b7} oneapi");
        name
    }

    fn cache_metrics_available(&self, model_id: &str) -> bool {
        let model = model_id.to_ascii_lowercase();
        !BAIDU_CLAUDE_FAMILY_NAMES
            .iter()
            .any(|name| model.contains(name))
    }

    fn uses_native_auth_headers(&self, def: &ProviderDefinition) -> bool {
        def.model_source == ProviderModelSource::BaiduOneApi
            && def.request_policy.effective_baidu_auth_bridge() == BaiduAuthBridge::DucxLoopback
    }

    fn uses_managed_auth_bridge(&self, def: &ProviderDefinition) -> bool {
        def.request_policy.effective_baidu_auth_bridge() == BaiduAuthBridge::DucxLoopback
    }

    fn reports_usage_telemetry(&self, def: &ProviderDefinition) -> bool {
        def.model_source == ProviderModelSource::BaiduOneApi && def.request_policy.baidu_code_report
    }

    fn auth_bridge_executable<'a>(&self, def: &'a ProviderDefinition) -> Option<&'a Path> {
        def.request_policy.ducx_executable.as_deref()
    }
}

pub(crate) static BAIDU_ONEAPI_ADAPTER: BaiduOneApiAdapter = BaiduOneApiAdapter;
