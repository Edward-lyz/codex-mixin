#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command as ProcessCommand;
use std::time::{Duration, SystemTime};

#[cfg(target_os = "macos")]
use super::{DoctorCheck, DoctorFix, DoctorStatus};

#[cfg(target_os = "macos")]
pub(super) fn check_desktop_apps(managed_config_path: &Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let config_mtime = std::fs::metadata(managed_config_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let mut any_running = false;
    for (app, fix) in [
        ("ChatGPT", DoctorFix::RestartChatGptApp),
        ("Codex", DoctorFix::RestartCodexApp),
    ] {
        let Some(pid) = desktop_app_pid(app) else {
            continue;
        };
        any_running = true;
        let id = format!("desktop_app_{}", app.to_lowercase());
        let name = format!("{app} App");
        match (process_start_time(pid), config_mtime) {
            (Some(started), Some(mtime)) if started < mtime => {
                checks.push(
                    DoctorCheck::new(
                        id,
                        name,
                        DoctorStatus::Warning,
                        format!("{app} started before the config update and is still using the old config; restart required"),
                    )
                    .detail(format!("pid {pid}; restart to load the latest model catalog"))
                    .hint(format!(
                        "osascript -e 'quit app \"{app}\"' && sleep 2 && open -a {app}"
                    ))
                    .fix(fix),
                );
            }
            (Some(_), _) => {
                checks.push(DoctorCheck::new(
                    id,
                    name,
                    DoctorStatus::Ok,
                    format!("{app} started after the config update and is using the latest config"),
                ));
            }
            (None, _) => {
                checks.push(
                    DoctorCheck::new(
                        id,
                        name,
                        DoctorStatus::Warning,
                        format!("{app} is running, but its start time could not be determined"),
                    )
                    .detail(format!("PID {pid}")),
                );
            }
        }
    }
    if !any_running {
        checks.push(DoctorCheck::new(
            "desktop_app",
            "Desktop App",
            DoctorStatus::Ok,
            "no running ChatGPT/Codex app detected; the next launch will load the latest config",
        ));
    }
    checks
}

/// Finds the main process of a desktop app bundle by matching the executable
/// path suffix, which is more reliable than `pgrep -x` for .app bundles.
#[cfg(target_os = "macos")]
pub(super) fn desktop_app_pid(app: &str) -> Option<u32> {
    let output = ProcessCommand::new("ps")
        .args(["-axo", "pid=,comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let suffix = format!("/{app}.app/Contents/MacOS/{app}");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let (pid, command) = line.trim_start().split_once(' ')?;
            command
                .trim()
                .ends_with(&suffix)
                .then(|| pid.parse().ok())
                .flatten()
        })
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> Option<SystemTime> {
    let output = ProcessCommand::new("ps")
        .args(["-p", &pid.to_string(), "-o", "etime="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let elapsed = parse_ps_etime(String::from_utf8_lossy(&output.stdout).trim())?;
    SystemTime::now().checked_sub(elapsed)
}

/// Parses `ps -o etime=` values shaped like `[[dd-]hh:]mm:ss`.
pub(super) fn parse_ps_etime(raw: &str) -> Option<Duration> {
    let (days, rest) = match raw.split_once('-') {
        Some((days, rest)) => (days.parse::<u64>().ok()?, rest),
        None => (0, raw),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let (hours, minutes, seconds): (u64, u64, u64) = match parts.as_slice() {
        [hours, minutes, seconds] => (
            hours.parse().ok()?,
            minutes.parse().ok()?,
            seconds.parse().ok()?,
        ),
        [minutes, seconds] => (0, minutes.parse().ok()?, seconds.parse().ok()?),
        _ => return None,
    };
    Some(Duration::from_secs(
        ((days * 24 + hours) * 60 + minutes) * 60 + seconds,
    ))
}
