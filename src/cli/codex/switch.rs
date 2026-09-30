use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::DocumentMut;

use super::{
    InstallCodexOptions, install_codex, resolve_codex_config_path,
    uninstall_codex_preserving_restore_mode,
};

const SWITCH_STATE_FILE: &str = "codex-switch.json";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CodexIntegration {
    Unmanaged,
    Managed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ManagedCodexMode {
    CodexOauthProxy,
    CustomOnly,
}

impl ManagedCodexMode {
    fn install_options(self) -> InstallCodexOptions {
        InstallCodexOptions {
            requested_model: None,
            set_default: self == Self::CustomOnly,
            codex_oauth_proxy: self == Self::CodexOauthProxy,
            custom_only: self == Self::CustomOnly,
            config_path: None,
            catalog_path: None,
            base_url: None,
            web_search: "live".to_owned(),
            env_key: None,
            no_env_key: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct CodexIntegrationStatus {
    pub(super) integration: CodexIntegration,
    pub(super) mode: Option<ManagedCodexMode>,
    pub(super) gateway_required: bool,
    pub(super) restore_mode: Option<ManagedCodexMode>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CodexSwitchState {
    restore_mode: ManagedCodexMode,
}

#[derive(Debug)]
pub(in crate::cli) struct CodexRequiresGatewayError;

impl fmt::Display for CodexRequiresGatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "Codex is connected through the Mixin gateway; run `codex-mixin codex-switch official` first, or pass --allow-codex-disconnect to force the stop",
        )
    }
}

impl std::error::Error for CodexRequiresGatewayError {}

pub(in crate::cli) fn codex_status(json_output: bool) -> anyhow::Result<()> {
    let status = current_codex_status()?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "Codex integration: {}",
            match status.integration {
                CodexIntegration::Managed => "managed",
                CodexIntegration::Unmanaged => "unmanaged",
            }
        );
        println!(
            "Mixin mode: {}",
            status.mode.map(mode_name).unwrap_or("none")
        );
        println!("Gateway required: {}", status.gateway_required);
        println!(
            "Restore mode: {}",
            status.restore_mode.map(mode_name).unwrap_or("none")
        );
    }
    Ok(())
}

pub(super) fn current_codex_status() -> anyhow::Result<CodexIntegrationStatus> {
    let config_path = resolve_codex_config_path(None)?;
    codex_status_from_paths(&config_path, &switch_state_path())
}

pub(in crate::cli) fn ensure_codex_allows_gateway_stop(
    allow_codex_disconnect: bool,
) -> anyhow::Result<()> {
    let config_path = resolve_codex_config_path(None)?;
    ensure_gateway_stop_allowed_at(&config_path, &switch_state_path(), allow_codex_disconnect)
}

fn ensure_gateway_stop_allowed_at(
    config_path: &Path,
    state_path: &Path,
    allow_codex_disconnect: bool,
) -> anyhow::Result<()> {
    // The override is the recovery path, so unreadable Codex or switch state
    // must not block it.
    if allow_codex_disconnect {
        return Ok(());
    }
    ensure_codex_allows_gateway_stop_status(&codex_status_from_paths(config_path, state_path)?)
}

fn ensure_codex_allows_gateway_stop_status(status: &CodexIntegrationStatus) -> anyhow::Result<()> {
    if status.gateway_required {
        return Err(CodexRequiresGatewayError.into());
    }
    Ok(())
}

pub(in crate::cli) async fn switch_to_official() -> anyhow::Result<()> {
    let before = current_codex_status()?;
    anyhow::ensure!(
        before.integration == CodexIntegration::Managed,
        "Codex is not currently managed by Codex Mixin"
    );
    let mode = before
        .mode
        .ok_or_else(|| anyhow::anyhow!("managed Codex mode could not be determined"))?;
    let previous_restore_mode = before.restore_mode;

    super::super::progress_step("Saving the current Codex integration mode");
    save_restore_mode(mode)?;
    super::super::progress_step("Restoring the official Codex configuration and authentication");
    if let Err(error) = uninstall_codex_preserving_restore_mode(None, None) {
        rollback_restore_mode_if_still_managed(previous_restore_mode)?;
        return Err(error);
    }
    codex_mixin::config::revoke_gateway_client_key(
        codex_mixin::gateway_access::GatewayClient::Codex,
    )?;

    super::super::progress_step("Validating the official Codex configuration");
    let restored = current_codex_status()?;
    anyhow::ensure!(
        restored.integration == CodexIntegration::Unmanaged && !restored.gateway_required,
        "Codex configuration is still managed; the gateway remains running"
    );

    super::super::progress_step("Stopping the Mixin gateway");
    super::super::service::stop_managed(true).await?;
    Ok(())
}

pub(in crate::cli) async fn switch_to_mixin() -> anyhow::Result<()> {
    let before = current_codex_status()?;
    if before.integration == CodexIntegration::Managed {
        clear_restore_mode()?;
        return Ok(());
    }
    let mode = before
        .restore_mode
        .ok_or_else(|| anyhow::anyhow!("no previous managed Codex mode is available to restore"))?;

    super::super::progress_step("Starting the Mixin gateway");
    super::super::service::ensure_ready().await?;
    super::super::progress_step("Restoring the previous Codex Mixin integration");
    install_codex(mode.install_options()).await?;
    super::super::progress_step("Validating the managed Codex configuration");
    let restored = current_codex_status()?;
    anyhow::ensure!(
        restored.integration == CodexIntegration::Managed
            && restored.mode == Some(mode)
            && restored.gateway_required,
        "Codex did not restore the previous Mixin integration mode"
    );
    Ok(())
}

pub(in crate::cli) fn clear_restore_mode() -> anyhow::Result<()> {
    clear_restore_mode_at(&switch_state_path())
}

fn switch_state_path() -> PathBuf {
    super::super::runtime::state_dir().join(SWITCH_STATE_FILE)
}

fn mode_name(mode: ManagedCodexMode) -> &'static str {
    match mode {
        ManagedCodexMode::CodexOauthProxy => "codex_oauth_proxy",
        ManagedCodexMode::CustomOnly => "custom_only",
    }
}

fn codex_status_from_paths(
    config_path: &Path,
    state_path: &Path,
) -> anyhow::Result<CodexIntegrationStatus> {
    let raw = if config_path.exists() {
        fs::read_to_string(config_path)?
    } else {
        String::new()
    };
    let managed = codex_mixin::clients::codex::document_is_managed(&raw);
    let mode = if managed { managed_mode(&raw) } else { None };
    Ok(CodexIntegrationStatus {
        integration: if managed {
            CodexIntegration::Managed
        } else {
            CodexIntegration::Unmanaged
        },
        mode,
        gateway_required: managed,
        restore_mode: load_restore_mode_at(state_path)?,
    })
}

fn managed_mode(raw: &str) -> Option<ManagedCodexMode> {
    let document = raw.parse::<DocumentMut>().ok()?;
    match codex_mixin::clients::codex::managed_provider_id(&document).ok()? {
        codex_mixin::CODEX_MIXIN_PROVIDER => Some(ManagedCodexMode::CodexOauthProxy),
        codex_mixin::clients::codex::CUSTOM_ONLY_PROVIDER
        | codex_mixin::clients::codex::LEGACY_CUSTOM_ONLY_PROVIDER => {
            Some(ManagedCodexMode::CustomOnly)
        }
        _ => None,
    }
}

fn save_restore_mode(mode: ManagedCodexMode) -> anyhow::Result<()> {
    let path = switch_state_path();
    let serialized = serde_json::to_vec_pretty(&CodexSwitchState { restore_mode: mode })?;
    codex_mixin::clients::files::write_owner_only(&path, &serialized)
}

fn load_restore_mode_at(path: &Path) -> anyhow::Result<Option<ManagedCodexMode>> {
    if !path.exists() {
        return Ok(None);
    }
    let state = serde_json::from_slice::<CodexSwitchState>(&fs::read(path)?)?;
    Ok(Some(state.restore_mode))
}

fn clear_restore_mode_at(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn rollback_restore_mode_if_still_managed(
    previous_restore_mode: Option<ManagedCodexMode>,
) -> anyhow::Result<()> {
    if current_codex_status()?.integration != CodexIntegration::Managed {
        return Ok(());
    }
    match previous_restore_mode {
        Some(mode) => save_restore_mode(mode),
        None => clear_restore_mode(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_uses_the_rust_managed_config_parser() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.toml");
        let state = directory.path().join(SWITCH_STATE_FILE);
        fs::write(
            &config,
            r#"model_provider = "codex-mixin"
[model_providers.codex-mixin]
base_url = "http://127.0.0.1:8787/v1"
"#,
        )
        .unwrap();

        let status = codex_status_from_paths(&config, &state).unwrap();

        assert_eq!(status.integration, CodexIntegration::Managed);
        assert_eq!(status.mode, Some(ManagedCodexMode::CodexOauthProxy));
        assert!(status.gateway_required);
        assert_eq!(status.restore_mode, None);
    }

    #[test]
    fn official_status_retains_the_mode_needed_for_reconnect() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.toml");
        let state = directory.path().join(SWITCH_STATE_FILE);
        fs::write(&config, "model_provider = \"openai\"\n").unwrap();
        fs::write(
            &state,
            serde_json::to_vec(&CodexSwitchState {
                restore_mode: ManagedCodexMode::CustomOnly,
            })
            .unwrap(),
        )
        .unwrap();

        let status = codex_status_from_paths(&config, &state).unwrap();

        assert_eq!(status.integration, CodexIntegration::Unmanaged);
        assert_eq!(status.mode, None);
        assert!(!status.gateway_required);
        assert_eq!(status.restore_mode, Some(ManagedCodexMode::CustomOnly));
    }

    #[test]
    fn unknown_managed_mode_still_requires_the_gateway() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.toml");
        let state = directory.path().join(SWITCH_STATE_FILE);
        fs::write(
            &config,
            format!(
                "{}\nmodel_provider = \"future-mixin\"\n",
                codex_mixin::clients::codex::MANAGED_HEADER
            ),
        )
        .unwrap();

        let status = codex_status_from_paths(&config, &state).unwrap();

        assert_eq!(status.integration, CodexIntegration::Managed);
        assert_eq!(status.mode, None);
        assert!(status.gateway_required);
    }

    #[test]
    fn clearing_restore_mode_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join(SWITCH_STATE_FILE);
        fs::write(&state, b"{}").unwrap();
        clear_restore_mode_at(&state).unwrap();
        clear_restore_mode_at(&state).unwrap();
        assert!(!state.exists());
    }

    #[test]
    fn managed_codex_blocks_gateway_stop() {
        let status = CodexIntegrationStatus {
            integration: CodexIntegration::Managed,
            mode: Some(ManagedCodexMode::CustomOnly),
            gateway_required: true,
            restore_mode: None,
        };

        let error = ensure_codex_allows_gateway_stop_status(&status).unwrap_err();
        assert!(error.downcast_ref::<CodexRequiresGatewayError>().is_some());
    }

    #[test]
    fn gateway_stop_allows_an_official_codex_configuration() {
        let status = CodexIntegrationStatus {
            integration: CodexIntegration::Unmanaged,
            mode: None,
            gateway_required: false,
            restore_mode: Some(ManagedCodexMode::CodexOauthProxy),
        };

        ensure_codex_allows_gateway_stop_status(&status).unwrap();
    }

    #[test]
    fn disconnect_override_does_not_depend_on_readable_switch_state() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.toml");
        let state = directory.path().join(SWITCH_STATE_FILE);
        fs::write(&state, b"not json").unwrap();

        ensure_gateway_stop_allowed_at(&config, &state, true).unwrap();
        assert!(ensure_gateway_stop_allowed_at(&config, &state, false).is_err());
    }
}
