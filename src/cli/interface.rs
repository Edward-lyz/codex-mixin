use codex_mixin::application::error::OperationError;
use codex_mixin::provider::{ProviderPreset, ProviderProtocol};
use serde_json::{Value, json};

pub(super) const PROTOCOL_VERSION: u32 = 1;

pub(super) fn describe(json_output: bool) -> anyhow::Result<()> {
    let paths = super::codex::resolve_codex_install_paths(None, None)?;
    let value = json!({
        "protocol_version": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "provider_catalog": provider_catalog(),
        "paths": {
            "state": std::path::absolute(super::runtime::state_dir())?,
            "config": std::path::absolute(codex_mixin::config::stored_config_path())?,
            "gateway_log": std::path::absolute(super::runtime::default_log_file_path())?,
            "report_log": std::path::absolute(super::runtime::default_report_hook_log_path())?,
            "codex_models_cache": std::path::absolute(paths.models_cache)?,
        },
        "capabilities": {
            "structured_errors": true,
            "agent_management": true,
            "provider_oauth_login": true,
            "agent_versions": true,
            "fusion_model_options": true,
            "service_lifecycle": true,
            "codex_lifecycle": true,
            "gateway_autostart": codex_mixin::platform::startup_service_supported(),
            "config_apply": true,
            "claude_desktop_routing": true,
            "official_ech_proxy": true,
            "cli_update": super::update::cli_release_target().is_ok(),
        },
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("CLI interface version: {PROTOCOL_VERSION}");
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}

fn provider_catalog() -> Value {
    let presets: Vec<_> = ProviderPreset::ALL
        .iter()
        .map(|preset| {
            let spec = preset.spec();
            let (auth_mode, credential_url) = match preset {
                ProviderPreset::Custom => ("api_key", None),
                ProviderPreset::Official | ProviderPreset::Agy => ("local_oauth", None),
                ProviderPreset::AwsBedrock => ("aws_sigv4", spec.website_url),
                ProviderPreset::BaiduOneApi => {
                    ("api_key", Some("https://oneapi-comate.baidu-int.com/token"))
                }
                ProviderPreset::OpenRouter => {
                    ("api_key", Some("https://openrouter.ai/settings/keys"))
                }
                ProviderPreset::DeepSeek => {
                    ("api_key", Some("https://platform.deepseek.com/api_keys"))
                }
                ProviderPreset::OpenCodeGo => ("api_key", Some("https://opencode.ai/go")),
            };
            json!({
                "id": spec.id,
                "default_id": preset.default_id(),
                "allow_multiple": *preset != ProviderPreset::Official,
                "display_name": spec.display_name,
                "description": spec.description,
                "icon": spec.icon,
                "protocol": spec.protocol,
                "auth_mode": auth_mode,
                "api_key_required": auth_mode == "api_key",
                "credential_url": credential_url,
                "custom_endpoint": *preset == ProviderPreset::Custom,
                "quota_username_required": *preset == ProviderPreset::BaiduOneApi,
                "quota_credentials_required": *preset == ProviderPreset::OpenCodeGo,
            })
        })
        .collect();
    let protocols: Vec<_> = [
        ProviderProtocol::OpenAiResponses,
        ProviderProtocol::AnthropicMessages,
        ProviderProtocol::OpenAiChat,
        ProviderProtocol::GoogleGemini,
    ]
    .into_iter()
    .map(|protocol| {
        let (name, path) = match protocol {
            ProviderProtocol::OpenAiResponses => ("Responses", "/v1/responses"),
            ProviderProtocol::AnthropicMessages => ("Messages", "/v1/messages"),
            ProviderProtocol::OpenAiChat => ("Chat Completions", "/v1/chat/completions"),
            ProviderProtocol::GoogleGemini => (
                "Google Gemini",
                "/v1beta/models/{model}:streamGenerateContent",
            ),
        };
        json!({"id": protocol, "display_name": name, "api_path": path})
    })
    .collect();
    json!({
        "default_preset": ProviderPreset::BaiduOneApi.as_str(),
        "default_protocol": ProviderProtocol::OpenAiResponses,
        "default_aws_region": codex_mixin::provider::AWS_BEDROCK_DEFAULT_REGION,
        "oauth_timeout_seconds": codex_mixin::provider::OAUTH_LOGIN_TIMEOUT_SECONDS,
        "presets": presets,
        "protocols": protocols,
    })
}

pub(super) fn command_error(error: &anyhow::Error) -> Value {
    let (code, committed, stage) = if error
        .downcast_ref::<super::codex::CodexRequiresGatewayError>()
        .is_some()
    {
        ("codex_requires_gateway", Some(false), None)
    } else {
        match error.downcast_ref::<OperationError>() {
            Some(OperationError::BeforeCommit { .. }) => ("before_commit", Some(false), None),
            Some(OperationError::AfterCommit { stage, .. }) => {
                ("after_commit", Some(true), Some(*stage))
            }
            None => ("operation_failed", None, None),
        }
    };
    json!({
        "protocol_version": PROTOCOL_VERSION,
        "error": {
            "code": code,
            "message": format!("{error:#}"),
            "committed": committed,
            "stage": stage,
        },
    })
}

pub(super) fn argument_error(message: &str) -> Value {
    json!({
        "protocol_version": PROTOCOL_VERSION,
        "error": {
            "code": "invalid_arguments",
            "message": message,
            "committed": false,
            "stage": null,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_envelope_preserves_commit_state_through_context() {
        let error = anyhow::Error::new(OperationError::AfterCommit {
            stage: "client synchronization",
            source: anyhow::anyhow!("permission denied"),
        })
        .context("provider update");
        let value = command_error(&error);
        assert_eq!(value["error"]["code"], "after_commit");
        assert_eq!(value["error"]["committed"], true);
        assert_eq!(value["error"]["stage"], "client synchronization");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("permission denied")
        );
    }

    #[test]
    fn unknown_errors_do_not_claim_rollback() {
        let value = command_error(&anyhow::anyhow!("unknown operation failure"));
        assert!(value["error"]["committed"].is_null());
    }

    #[test]
    fn gateway_guard_has_a_stable_machine_error_code() {
        let error = anyhow::Error::new(super::super::codex::CodexRequiresGatewayError);
        let value = command_error(&error);
        assert_eq!(value["error"]["code"], "codex_requires_gateway");
        assert_eq!(value["error"]["committed"], false);
    }
}
