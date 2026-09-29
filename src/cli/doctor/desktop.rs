use std::path::Path;

use codex_mixin::platform::{codex_desktop_apps, desktop_app_restart_hint, running_desktop_app};

use super::{DoctorCheck, DoctorFix, DoctorStatus};

/// Report whether each running Codex desktop app started after the managed
/// config was last written, i.e. whether it already loaded the new catalog.
/// Platforms without Codex desktop apps report nothing.
pub(super) fn check_desktop_apps(managed_config_path: &Path) -> Vec<DoctorCheck> {
    let apps = codex_desktop_apps();
    if apps.is_empty() {
        return Vec::new();
    }
    let mut checks = Vec::new();
    let config_mtime = std::fs::metadata(managed_config_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    for &app in apps {
        let Some(running) = running_desktop_app(app) else {
            continue;
        };
        let id = format!("desktop_app_{}", app.to_lowercase());
        let name = format!("{app} App");
        let pid = running.pid;
        checks.push(match (running.started_at, config_mtime) {
            (Some(started), Some(mtime)) if started < mtime => {
                let check = DoctorCheck::new(
                    id,
                    name,
                    DoctorStatus::Warning,
                    format!("{app} started before the config update and is still using the old config; restart required"),
                )
                .detail(format!("pid {pid}; restart to load the latest model catalog"))
                .hint(desktop_app_restart_hint(app));
                match DoctorFix::restart_desktop_app(app) {
                    Some(fix) => check.fix(fix),
                    None => check,
                }
            }
            (Some(_), _) => DoctorCheck::new(
                id,
                name,
                DoctorStatus::Ok,
                format!("{app} started after the config update and is using the latest config"),
            ),
            (None, _) => DoctorCheck::new(
                id,
                name,
                DoctorStatus::Warning,
                format!("{app} is running, but its start time could not be determined"),
            )
            .detail(format!("PID {pid}")),
        });
    }
    if checks.is_empty() {
        checks.push(DoctorCheck::new(
            "desktop_app",
            "Desktop App",
            DoctorStatus::Ok,
            format!(
                "no running {} app detected; the next launch will load the latest config",
                apps.join("/")
            ),
        ));
    }
    checks
}
