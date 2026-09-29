use codex_mixin::application::error::OperationError;
use serde_json::{Value, json};

pub(super) const PROTOCOL_VERSION: u32 = 1;

pub(super) fn describe(json_output: bool) -> anyhow::Result<()> {
    let paths = super::codex::resolve_codex_install_paths(None, None)?;
    let value = json!({
        "protocol_version": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "paths": {
            "state": std::path::absolute(super::runtime::state_dir())?,
            "config": std::path::absolute(codex_mixin::config::stored_config_path())?,
            "gateway_log": std::path::absolute(super::runtime::default_log_file_path())?,
            "report_log": std::path::absolute(super::runtime::default_report_hook_log_path())?,
            "codex_models_cache": std::path::absolute(paths.models_cache)?,
        },
        "capabilities": {
            "structured_errors": true,
            "fusion_model_options": true,
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

pub(super) fn command_error(error: &anyhow::Error) -> Value {
    let (code, committed, stage) = match error.downcast_ref::<OperationError>() {
        Some(OperationError::BeforeCommit { .. }) => ("before_commit", Some(false), None),
        Some(OperationError::AfterCommit { stage, .. }) => {
            ("after_commit", Some(true), Some(*stage))
        }
        None => ("operation_failed", None, None),
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
}
