use codex_mixin::config::GatewayConfig;
use codex_mixin::config::stored_config_path;

use super::super::codex::{
    refresh_default_managed_codex_catalog, resolve_codex_config_path,
    sync_managed_codex_gateway_base_url,
};
use super::super::runtime::{
    delete_daemon_metadata, delete_runtime_metadata, load_daemon_metadata, load_runtime_metadata,
    pid_is_running,
};
use super::super::service::start_daemon;
use super::{DoctorFix, RepairOutcome};

pub(super) async fn apply_doctor_fixes(fixes: &[DoctorFix]) -> Vec<RepairOutcome> {
    let mut outcomes = Vec::new();
    for &fix in fixes {
        println!("auto-fix: {} ...", fix.description());
        let result = apply_doctor_fix(fix).await;
        let (ok, message) = match result {
            Ok(message) => (true, message),
            Err(error) => (false, format!("{error:#}")),
        };
        println!(
            "auto-fix: {} => {}",
            fix.description(),
            if ok { &message } else { "failed" }
        );
        outcomes.push(RepairOutcome {
            fix,
            description: fix.description().to_owned(),
            ok,
            message,
        });
    }
    outcomes
}

async fn apply_doctor_fix(fix: DoctorFix) -> anyhow::Result<String> {
    match fix {
        DoctorFix::CleanStaleGatewayMetadata => {
            let mut removed = Vec::new();
            if let Some(runtime) = load_runtime_metadata()?
                && !pid_is_running(runtime.pid)?
            {
                delete_runtime_metadata()?;
                removed.push("runtime.json");
            }
            if let Some(daemon) = load_daemon_metadata()?
                && !pid_is_running(daemon.pid)?
            {
                delete_daemon_metadata()?;
                removed.push("daemon.json");
            }
            if removed.is_empty() {
                Ok("no stale runtime metadata to remove".to_owned())
            } else {
                Ok(format!("removed {}", removed.join(", ")))
            }
        }
        DoctorFix::StartGateway => {
            let config = GatewayConfig::from_stored_config()?;
            tokio::task::spawn_blocking(move || start_daemon(None, None, &config, false)).await??;
            let bind = load_runtime_metadata()?
                .map(|runtime| runtime.bind.to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            Ok(format!("gateway started on {bind}"))
        }
        DoctorFix::FixConfigPermissions => {
            let path = stored_config_path();
            codex_mixin::platform::restrict_owner_only_file(&path)?;
            Ok(format!("owner-only access applied to {}", path.display()))
        }
        DoctorFix::SyncGatewayBaseUrl => {
            let config_path = resolve_codex_config_path(None)?;
            let runtime = load_runtime_metadata()?
                .filter(|runtime| pid_is_running(runtime.pid).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("gateway is not running; cannot sync base_url"))?;
            let changed = sync_managed_codex_gateway_base_url(&config_path, runtime.bind)?;
            Ok(if changed {
                format!("base_url updated to http://{}/v1", runtime.bind)
            } else {
                "base_url is already current".to_owned()
            })
        }
        DoctorFix::RefreshCodexCatalog => {
            refresh_default_managed_codex_catalog().await?;
            let refreshed_clients = crate::cli::sync_installed_client_models()?;
            Ok(if refreshed_clients.is_empty() {
                "managed model catalog refreshed".to_owned()
            } else {
                format!(
                    "managed model catalog refreshed; client models refreshed: {}",
                    refreshed_clients.join(", ")
                )
            })
        }
        DoctorFix::RestartChatGptApp | DoctorFix::RestartCodexApp => {
            let app = fix
                .desktop_app()
                .ok_or_else(|| anyhow::anyhow!("fix does not name a desktop app"))?;
            tokio::task::spawn_blocking(move || codex_mixin::platform::restart_desktop_app(app))
                .await??;
            Ok(format!("{app} restarted"))
        }
    }
}
