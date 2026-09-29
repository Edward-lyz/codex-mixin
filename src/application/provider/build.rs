use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::provider::{
    AWS_BEDROCK_DEFAULT_REGION, ProviderDefinition, ProviderModel, ProviderModelSource,
    ProviderPreset, ProviderQuotaParser, aws_bedrock_aksk_provider,
};

use super::super::error::OperationError;
use super::discovery::{InferredCustomProviderEndpoint, infer_custom_provider_endpoint};
use super::{provider_for_refresh, update_provider};

#[derive(Clone, Debug, Default)]
pub struct AddProviderInput {
    pub preset: String,
    pub id: Option<String>,
    pub key: Option<String>,
    pub aws_access_key_id: Option<String>,
    pub aws_secret_access_key: Option<String>,
    pub aws_session_token: Option<String>,
    pub aws_region: Option<String>,
    pub display_name: Option<String>,
    pub base_url: Option<String>,
    pub website_url: Option<String>,
    pub protocol: Option<String>,
    pub api_path: Option<String>,
    pub models_path: Option<String>,
    pub image_generation_path: Option<String>,
    pub quota_url: Option<String>,
    pub quota_username: Option<String>,
    pub quota_workspace_id: Option<String>,
    pub quota_auth_cookie: Option<String>,
    pub quota_currency: Option<String>,
    pub quota_parser: Option<String>,
    pub gateway_key: Option<String>,
    pub static_models: Vec<String>,
    pub header_env: Vec<String>,
    pub baidu_auth_bridge: Option<String>,
    pub ducx_executable: Option<PathBuf>,
    pub baidu_code_report: Option<bool>,
    pub auxiliary_model_upstream: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct UpdateProviderInput {
    pub id: String,
    pub auxiliary_model_upstream: Option<bool>,
    pub key: Option<String>,
    pub clear_key: bool,
    pub aws_access_key_id: Option<String>,
    pub aws_secret_access_key: Option<String>,
    pub aws_session_token: Option<String>,
    pub aws_region: Option<String>,
    pub clear_aws_session_token: bool,
    pub clear_aws_credentials: bool,
    pub display_name: Option<String>,
    pub base_url: Option<String>,
    pub website_url: Option<String>,
    pub protocol: Option<String>,
    pub api_path: Option<String>,
    pub models_path: Option<String>,
    pub image_generation_path: Option<String>,
    pub clear_image_generation: bool,
    pub quota_url: Option<String>,
    pub clear_quota: bool,
    pub quota_username: Option<String>,
    pub quota_workspace_id: Option<String>,
    pub clear_quota_workspace_id: bool,
    pub quota_auth_cookie: Option<String>,
    pub clear_quota_auth_cookie: bool,
    pub quota_currency: Option<String>,
    pub quota_parser: Option<String>,
    pub header_env: Vec<String>,
    pub clear_header_env: bool,
    pub baidu_auth_bridge: Option<String>,
    pub ducx_executable: Option<PathBuf>,
    pub baidu_code_report: Option<bool>,
    pub auto_review_model: Option<String>,
    pub clear_auto_review_model: bool,
}

#[derive(Debug)]
pub struct PreparedProvider {
    pub id: String,
    pub protocol: crate::provider::ProviderProtocol,
    pub provider: ProviderDefinition,
    pub gateway_api_key: Option<String>,
    pub should_probe_protocol: bool,
    pub should_refresh_capabilities: bool,
}

pub fn create_provider(input: AddProviderInput) -> anyhow::Result<PreparedProvider> {
    let preset = ProviderPreset::parse(input.preset.trim())?;
    let id = input.id.unwrap_or_else(|| preset.default_id().to_owned());
    let mut provider = if preset == ProviderPreset::AwsBedrock && input.key.is_none() {
        let access_key_id = required(
            "AWS access key ID",
            input
                .aws_access_key_id
                .ok_or_else(|| anyhow::anyhow!("aws-bedrock requires --aws-access-key-id"))?,
        )?;
        let secret_access_key = required(
            "AWS secret access key",
            input
                .aws_secret_access_key
                .ok_or_else(|| anyhow::anyhow!("aws-bedrock requires --aws-secret-access-key"))?,
        )?;
        let session_token = input
            .aws_session_token
            .map(|value| required("AWS session token", value))
            .transpose()?;
        let region = required(
            "AWS region",
            input
                .aws_region
                .unwrap_or_else(|| AWS_BEDROCK_DEFAULT_REGION.to_owned()),
        )?;
        aws_bedrock_aksk_provider(
            id.clone(),
            access_key_id,
            secret_access_key,
            session_token,
            region,
        )
    } else {
        anyhow::ensure!(
            input.aws_access_key_id.is_none()
                && input.aws_secret_access_key.is_none()
                && input.aws_session_token.is_none()
                && input.aws_region.is_none(),
            "AWS credential options require the aws-bedrock preset without --key"
        );
        preset.create(
            id.clone(),
            required(
                "key",
                input
                    .key
                    .ok_or_else(|| anyhow::anyhow!("provider requires --key"))?,
            )?,
        )
    };
    if let Some(name) = input.display_name {
        provider.display_name = required("display name", name)?;
    }
    let explicit_protocol = input.protocol.is_some();
    let explicit_api_path = input.api_path.is_some();
    let explicit_models_path = input.models_path.is_some();
    let has_static_models = !input.static_models.is_empty();
    let inferred = if preset == ProviderPreset::Custom {
        input
            .base_url
            .as_deref()
            .map(infer_custom_provider_endpoint)
            .transpose()?
    } else {
        None
    };
    let path_explicit = inferred
        .as_ref()
        .is_some_and(|endpoint| endpoint.path_explicit);
    if let Some(endpoint) = inferred {
        apply_endpoint(&mut provider, endpoint);
    } else if let Some(url) = input.base_url {
        provider.base_url = normalize_base_url(url)?;
    }
    if let Some(url) = input.website_url {
        provider.website_url = Some(normalize_base_url(url)?);
    }
    anyhow::ensure!(
        preset != ProviderPreset::Custom || !provider.base_url.is_empty(),
        "custom provider requires --base-url"
    );
    if let Some(protocol) = input.protocol {
        provider.protocol = parse_protocol(&protocol)?;
    }
    if let Some(path) = input.api_path {
        provider.api_path = normalize_path("API path", path)?;
    }
    if let Some(path) = input.models_path {
        provider.model_source = ProviderModelSource::OpenAiCompatible {
            path: normalize_path("models path", path)?,
        };
    }
    if !input.static_models.is_empty() {
        let models = normalize_model_ids(input.static_models)?;
        provider.model_source = ProviderModelSource::Static;
        provider.cached_models = models
            .iter()
            .map(|id| ProviderModel {
                id: id.clone(),
                ..ProviderModel::default()
            })
            .collect();
        provider.selected_models = models;
    }
    if let Some(path) = input.image_generation_path {
        provider.image_generation_path = Some(normalize_path("image generation path", path)?);
    }
    if let Some(url) = input.quota_url {
        provider.quota_url = Some(normalize_base_url(url)?);
    }
    let opencode_quota_fields =
        input.quota_workspace_id.is_some() || input.quota_auth_cookie.is_some();
    if let Some(value) = input.quota_username {
        provider.quota_username = Some(required("quota username", value)?);
    }
    if let Some(value) = input.quota_workspace_id {
        provider.quota_workspace_id = Some(required("quota workspace ID", value)?);
    }
    if let Some(value) = input.quota_auth_cookie {
        provider.quota_auth_cookie = Some(required("quota auth cookie", value)?);
    }
    if let Some(value) = input.quota_currency {
        provider.quota_currency = Some(normalize_currency(value)?);
    }
    if let Some(value) = input.quota_parser {
        provider.quota_parser = parse_quota_parser(&value)?;
    }
    if provider.preset_id.as_deref() == Some("opencode-go") && opencode_quota_fields {
        provider.quota_parser = ProviderQuotaParser::OpenCodeGo;
        provider.quota_currency = Some("USD".to_owned());
    }
    provider.request_policy.custom_headers_from_env = parse_header_env(&input.header_env)?;
    apply_baidu_options(
        &mut provider,
        input.baidu_auth_bridge.as_deref(),
        input.ducx_executable,
    )?;
    if let Some(enabled) = input.baidu_code_report {
        provider.request_policy.baidu_code_report = enabled;
    }
    set_report_sibling(&mut provider);
    provider.auxiliary_model_upstream = input.auxiliary_model_upstream.unwrap_or(false);
    let should_probe_protocol = preset == ProviderPreset::Custom
        && !explicit_protocol
        && !explicit_api_path
        && !explicit_models_path
        && !path_explicit
        && !has_static_models;
    provider.validate()?;
    let gateway_api_key = input
        .gateway_key
        .map(|value| required("gateway key", value))
        .transpose()?;
    let protocol = provider.protocol;
    Ok(PreparedProvider {
        id,
        protocol,
        provider,
        gateway_api_key,
        should_probe_protocol,
        should_refresh_capabilities: true,
    })
}

pub fn update_provider_from_input(
    input: UpdateProviderInput,
) -> Result<PreparedProvider, OperationError> {
    let id = input.id.clone();
    let refresh = input.key.is_some()
        || input.base_url.is_some()
        || input.protocol.is_some()
        || input.api_path.is_some()
        || input.models_path.is_some()
        || !input.header_env.is_empty();
    let header_env = parse_header_env(&input.header_env).map_err(before_commit)?;
    let explicit_protocol = input.protocol.is_some();
    let explicit_api_path = input.api_path.is_some();
    let explicit_models_path = input.models_path.is_some();
    let base_url_updated = input.base_url.is_some();
    let snapshot = provider_for_refresh(&id).map_err(before_commit)?;
    let mut provider = snapshot.clone();
    if input.clear_key {
        provider.auth.api_key.clear();
    } else if let Some(key) = &input.key {
        provider.auth.api_key = required("key", key.clone()).map_err(before_commit)?;
        provider.auth.aws_sigv4 = None;
    }
    apply_aws_auth_options(&mut provider, &input).map_err(before_commit)?;
    apply_quota_update(&mut provider, &input).map_err(before_commit)?;
    if let Some(name) = input.display_name {
        provider.display_name = required("display name", name).map_err(before_commit)?;
    }
    let inferred = if provider.preset_id.as_deref() == Some("custom") {
        input
            .base_url
            .as_deref()
            .map(infer_custom_provider_endpoint)
            .transpose()
            .map_err(before_commit)?
    } else {
        None
    };
    let path_explicit = inferred
        .as_ref()
        .is_some_and(|endpoint| endpoint.path_explicit);
    if let Some(endpoint) = inferred {
        apply_endpoint(&mut provider, endpoint);
    } else if let Some(url) = input.base_url {
        provider.base_url = normalize_base_url(url).map_err(before_commit)?;
    }
    if let Some(url) = input.website_url {
        provider.website_url = if url.trim().is_empty() {
            None
        } else {
            Some(normalize_base_url(url).map_err(before_commit)?)
        };
    }
    if let Some(protocol) = input.protocol {
        provider.protocol = parse_protocol(&protocol).map_err(before_commit)?;
    }
    if let Some(path) = input.api_path {
        provider.api_path = normalize_path("API path", path).map_err(before_commit)?;
    }
    let should_probe_protocol = provider.preset_id.as_deref() == Some("custom")
        && base_url_updated
        && !explicit_protocol
        && !explicit_api_path
        && !explicit_models_path
        && !path_explicit;
    if let Some(path) = input.models_path {
        provider.model_source = ProviderModelSource::OpenAiCompatible {
            path: normalize_path("models path", path).map_err(before_commit)?,
        };
    }
    if input.clear_image_generation {
        provider.image_generation_path = None;
    } else if let Some(path) = input.image_generation_path {
        provider.image_generation_path =
            Some(normalize_path("image generation path", path).map_err(before_commit)?);
    }
    if input.clear_header_env {
        provider.request_policy.custom_headers_from_env.clear();
    }
    provider
        .request_policy
        .custom_headers_from_env
        .extend(header_env);
    apply_baidu_options(
        &mut provider,
        input.baidu_auth_bridge.as_deref(),
        input.ducx_executable,
    )
    .map_err(before_commit)?;
    if let Some(enabled) = input.baidu_code_report {
        provider.request_policy.baidu_code_report = enabled;
    }
    set_report_sibling(&mut provider);
    if input.clear_auto_review_model {
        provider.auto_review_model = None;
    } else if let Some(model) = input.auto_review_model {
        provider.auto_review_model =
            Some(required("auto review model", model).map_err(before_commit)?);
    }
    provider.validate().map_err(before_commit)?;
    let protocol = provider.protocol;
    update_provider(
        &id,
        &snapshot,
        provider.clone(),
        input.auxiliary_model_upstream,
    )?;
    Ok(PreparedProvider {
        id,
        protocol,
        provider,
        gateway_api_key: None,
        should_probe_protocol,
        should_refresh_capabilities: refresh,
    })
}

fn apply_quota_update(
    provider: &mut ProviderDefinition,
    input: &UpdateProviderInput,
) -> anyhow::Result<()> {
    if input.clear_quota {
        provider.quota_url = None;
        provider.quota_username = None;
        provider.quota_workspace_id = None;
        provider.quota_auth_cookie = None;
        provider.quota_currency = None;
        provider.quota_parser = match provider.preset_id.as_deref() {
            Some("deepseek") => ProviderQuotaParser::DeepSeek,
            Some("opencode-go") => ProviderQuotaParser::OpenCodeGo,
            _ => ProviderQuotaParser::Generic,
        };
    } else {
        if let Some(url) = &input.quota_url {
            provider.quota_url = Some(normalize_base_url(url.clone())?);
        }
        if let Some(value) = &input.quota_username {
            provider.quota_username = Some(required("quota username", value.clone())?);
        }
        let opencode_quota_fields =
            input.quota_workspace_id.is_some() || input.quota_auth_cookie.is_some();
        if input.clear_quota_workspace_id {
            provider.quota_workspace_id = None;
        } else if let Some(value) = &input.quota_workspace_id {
            provider.quota_workspace_id = Some(required("quota workspace ID", value.clone())?);
        }
        if input.clear_quota_auth_cookie {
            provider.quota_auth_cookie = None;
        } else if let Some(value) = &input.quota_auth_cookie {
            provider.quota_auth_cookie = Some(required("quota auth cookie", value.clone())?);
        }
        if let Some(value) = &input.quota_currency {
            provider.quota_currency = Some(normalize_currency(value.clone())?);
        }
        if let Some(value) = &input.quota_parser {
            provider.quota_parser = parse_quota_parser(value)?;
        }
        if provider.preset_id.as_deref() == Some("opencode-go") && opencode_quota_fields {
            provider.quota_parser = ProviderQuotaParser::OpenCodeGo;
            provider.quota_currency = Some("USD".to_owned());
        }
    }
    Ok(())
}

fn before_commit(source: anyhow::Error) -> OperationError {
    OperationError::BeforeCommit { source }
}

fn required(label: &str, value: String) -> anyhow::Result<String> {
    let trimmed = value.trim().to_owned();
    anyhow::ensure!(!trimmed.is_empty(), "{label} cannot be empty");
    Ok(trimmed)
}

fn normalize_base_url(value: String) -> anyhow::Result<String> {
    let mut value = required("base URL", value)?;
    while value.ends_with('/') {
        value.pop();
    }
    anyhow::ensure!(
        value.starts_with("http://") || value.starts_with("https://"),
        "base URL must start with http:// or https://"
    );
    Ok(value)
}

fn normalize_path(label: &str, value: String) -> anyhow::Result<String> {
    let value = required(label, value)?;
    Ok(if value.starts_with('/') {
        value
    } else {
        format!("/{value}")
    })
}

fn normalize_currency(value: String) -> anyhow::Result<String> {
    let value = required("quota currency", value)?.to_ascii_uppercase();
    anyhow::ensure!(
        value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase()),
        "quota currency must be a three-letter code"
    );
    Ok(value)
}

fn normalize_model_ids(values: Vec<String>) -> anyhow::Result<Vec<String>> {
    let mut models = Vec::with_capacity(values.len());
    for value in values {
        let value = required("model", value)?;
        if !models.contains(&value) {
            models.push(value);
        }
    }
    Ok(models)
}

fn parse_protocol(value: &str) -> anyhow::Result<crate::provider::ProviderProtocol> {
    match value.trim() {
        "anthropic_messages" | "anthropic" => {
            Ok(crate::provider::ProviderProtocol::AnthropicMessages)
        }
        "open_ai_chat" | "openai_chat" | "chat" => {
            Ok(crate::provider::ProviderProtocol::OpenAiChat)
        }
        "open_ai_responses" | "openai_responses" | "responses" => {
            Ok(crate::provider::ProviderProtocol::OpenAiResponses)
        }
        other => anyhow::bail!("unsupported provider protocol: {other}"),
    }
}

fn parse_quota_parser(value: &str) -> anyhow::Result<ProviderQuotaParser> {
    match value.trim() {
        "generic" => Ok(ProviderQuotaParser::Generic),
        "baidu_oneapi" | "baidu-oneapi" => Ok(ProviderQuotaParser::BaiduOneApi),
        "openrouter" => Ok(ProviderQuotaParser::OpenRouter),
        "deepseek" => Ok(ProviderQuotaParser::DeepSeek),
        "opencode_go" | "opencode-go" => Ok(ProviderQuotaParser::OpenCodeGo),
        other => anyhow::bail!("unsupported quota parser: {other}"),
    }
}

fn parse_header_env(values: &[String]) -> anyhow::Result<BTreeMap<String, String>> {
    values
        .iter()
        .map(|value| {
            let (header, variable) = value.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("custom header mapping must use NAME=ENV_VAR: {value}")
            })?;
            Ok((header.trim().to_owned(), variable.trim().to_owned()))
        })
        .collect()
}

fn apply_baidu_options(
    provider: &mut ProviderDefinition,
    bridge: Option<&str>,
    executable: Option<PathBuf>,
) -> anyhow::Result<()> {
    if let Some(bridge) = bridge {
        provider.request_policy.baidu_auth_bridge = Some(match bridge {
            "disabled" => crate::provider::BaiduAuthBridge::Disabled,
            "ducx_loopback" => crate::provider::BaiduAuthBridge::DucxLoopback,
            other => anyhow::bail!(
                "invalid Baidu auth bridge {other}; expected disabled or ducx_loopback"
            ),
        });
    }
    if let Some(executable) = executable {
        provider.request_policy.ducx_executable = Some(executable);
    }
    Ok(())
}

fn set_report_sibling(provider: &mut ProviderDefinition) {
    if provider.request_policy.baidu_code_report
        && provider.request_policy.data_report_executable.is_none()
    {
        provider.request_policy.data_report_executable = provider
            .request_policy
            .ducx_executable
            .as_deref()
            .and_then(|executable| {
                let install = executable.parent()?.parent()?;
                Some(install.join(crate::provider::auth::ducx::data_report_relative_path()))
            });
    }
}

fn apply_endpoint(provider: &mut ProviderDefinition, endpoint: InferredCustomProviderEndpoint) {
    super::discovery::apply_inferred_custom_endpoint(provider, endpoint);
}

fn apply_aws_auth_options(
    provider: &mut ProviderDefinition,
    input: &UpdateProviderInput,
) -> anyhow::Result<()> {
    use crate::provider::{
        AWS_BEDROCK_RUNTIME_SERVICE, AwsSigV4AuthConfig, aws_bedrock_runtime_base_url,
    };
    let has_options = input.aws_access_key_id.is_some()
        || input.aws_secret_access_key.is_some()
        || input.aws_session_token.is_some()
        || input.aws_region.is_some()
        || input.clear_aws_session_token
        || input.clear_aws_credentials;
    if !has_options {
        return Ok(());
    }
    anyhow::ensure!(
        provider.preset_id.as_deref() == Some("aws-bedrock"),
        "AWS credential options require an aws-bedrock provider"
    );
    if input.clear_aws_credentials {
        provider.auth.aws_sigv4 = None;
        return Ok(());
    }
    let mut aws = provider
        .auth
        .aws_sigv4
        .take()
        .unwrap_or(AwsSigV4AuthConfig {
            access_key_id: String::new(),
            secret_access_key: String::new(),
            session_token: None,
            region: AWS_BEDROCK_DEFAULT_REGION.to_owned(),
            service: AWS_BEDROCK_RUNTIME_SERVICE.to_owned(),
        });
    if let Some(value) = &input.aws_access_key_id {
        aws.access_key_id = required("AWS access key ID", value.clone())?;
    }
    if let Some(value) = &input.aws_secret_access_key {
        aws.secret_access_key = required("AWS secret access key", value.clone())?;
    }
    if input.clear_aws_session_token {
        aws.session_token = None;
    } else if let Some(value) = &input.aws_session_token {
        aws.session_token = Some(required("AWS session token", value.clone())?);
    }
    if let Some(value) = &input.aws_region {
        aws.region = required("AWS region", value.clone())?;
        if input.base_url.is_none() {
            provider.base_url = aws_bedrock_runtime_base_url(&aws.region);
        }
    }
    anyhow::ensure!(
        !aws.access_key_id.trim().is_empty(),
        "AWS access key ID is required"
    );
    anyhow::ensure!(
        !aws.secret_access_key.trim().is_empty(),
        "AWS secret access key is required"
    );
    provider.auth.api_key.clear();
    provider.auth.aws_sigv4 = Some(aws);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_provider_endpoint_inference_preserves_explicit_api_suffix() {
        let endpoint =
            infer_custom_provider_endpoint("https://example.test/v1/chat/completions").unwrap();
        assert_eq!(endpoint.base_url, "https://example.test");
        assert_eq!(
            endpoint.protocol,
            crate::provider::ProviderProtocol::OpenAiChat
        );
        assert!(endpoint.path_explicit);
    }

    #[test]
    fn custom_provider_requires_base_url_and_nonempty_credentials() {
        let missing_url = create_provider(AddProviderInput {
            preset: "custom".to_owned(),
            key: Some("key".to_owned()),
            ..AddProviderInput::default()
        });
        assert!(missing_url.unwrap_err().to_string().contains("base-url"));
        let blank_key = create_provider(AddProviderInput {
            preset: "deepseek".to_owned(),
            key: Some("  ".to_owned()),
            ..AddProviderInput::default()
        });
        assert!(
            blank_key
                .unwrap_err()
                .to_string()
                .contains("key cannot be empty")
        );
    }

    #[test]
    fn baidu_oneapi_add_without_bridge_leaves_loopback_unset() {
        let mut provider = crate::provider::baidu_oneapi_provider("baidu-oneapi", "key");

        apply_baidu_options(&mut provider, None, None).unwrap();

        assert_eq!(provider.request_policy.baidu_auth_bridge, None);
    }

    #[test]
    fn provider_mutations_persist_managed_ducx_options() {
        let mut provider = crate::provider::baidu_oneapi_provider("baidu-oneapi", "key");
        provider.quota_username = Some("user@example.com".to_owned());
        let install =
            std::env::temp_dir().join("example/.codex-mixin/ducx/home/.baidu-cx/baidu-cx");
        let executable = install
            .join("bin")
            .join(crate::platform::executable_file_name("ducx"));

        apply_baidu_options(
            &mut provider,
            Some("ducx_loopback"),
            Some(executable.clone()),
        )
        .unwrap();

        assert_eq!(
            provider.request_policy.baidu_auth_bridge,
            Some(crate::provider::BaiduAuthBridge::DucxLoopback)
        );
        assert_eq!(provider.request_policy.ducx_executable, Some(executable));
        provider.request_policy.baidu_code_report = true;
        set_report_sibling(&mut provider);
        assert_eq!(
            provider.request_policy.data_report_executable,
            Some(
                install
                    .join("hooks")
                    .join(crate::platform::executable_file_name("data-report"))
            )
        );
        provider.validate().unwrap();
    }

    #[test]
    fn parses_custom_header_environment_mappings() {
        let mapping = parse_header_env(&[
            "x-example-auth=EXAMPLE_AUTH".to_owned(),
            "x-routing-token=ROUTING_TOKEN".to_owned(),
        ])
        .unwrap();

        assert_eq!(mapping["x-example-auth"], "EXAMPLE_AUTH");
        assert_eq!(mapping["x-routing-token"], "ROUTING_TOKEN");
        assert!(parse_header_env(&["missing-separator".to_owned()]).is_err());
    }

    #[test]
    fn parses_opencode_go_quota_parser() {
        assert_eq!(
            parse_quota_parser("opencode_go").unwrap(),
            ProviderQuotaParser::OpenCodeGo
        );
        assert_eq!(
            parse_quota_parser("opencode-go").unwrap(),
            ProviderQuotaParser::OpenCodeGo
        );
    }
}
