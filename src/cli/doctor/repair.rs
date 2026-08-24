#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use codex_mixin::config::{GatewayConfig, stored_config_path};

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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let path = stored_config_path();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
                Ok(format!("chmod 600 applied to {}", path.display()))
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!("automatic permission repair is not supported on this platform")
            }
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
            Ok("managed model catalog refreshed".to_owned())
        }
        #[cfg(target_os = "macos")]
        DoctorFix::RestartChatGptApp => restart_app_fix("ChatGPT").await,
        #[cfg(target_os = "macos")]
        DoctorFix::RestartCodexApp => restart_app_fix("Codex").await,
    }
}

#[cfg(target_os = "macos")]
async fn restart_app_fix(app: &'static str) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || restart_macos_app(app)).await?
}

#[cfg(target_os = "macos")]
fn restart_macos_app(app: &str) -> anyhow::Result<String> {
    let previous_pid = super::desktop::desktop_app_pid(app);
    let quit = std::process::Command::new("osascript")
        .args(["-e", &format!("quit app \"{app}\"")])
        .status()?;
    if !quit.success() {
        anyhow::bail!("failed to quit {app}");
    }
    match previous_pid {
        Some(pid) => {
            let deadline = Instant::now() + Duration::from_secs(15);
            while super::super::runtime::pid_is_running(pid).unwrap_or(false) {
                if Instant::now() >= deadline {
                    anyhow::bail!(
                        "{app} did not exit within 15s (a window may be blocking quit); restart it manually"
                    );
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        None => std::thread::sleep(Duration::from_secs(2)),
    }
    let open = std::process::Command::new("open")
        .args(["-a", app])
        .status()?;
    if !open.success() {
        anyhow::bail!("failed to reopen {app}");
    }
    Ok(format!("{app} restarted"))
}
