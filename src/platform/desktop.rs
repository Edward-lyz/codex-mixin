//! Desktop-session services: user notifications and the Codex desktop apps
//! that load the managed configuration.
//!
//! Functions here block on OS tools; async callers use `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A user-visible desktop notification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesktopNotification {
    pub title: String,
    pub subtitle: String,
    pub body: String,
}

/// Outcome of a notification request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NotificationDelivery {
    Delivered,
    /// This OS session has no notification service the CLI can drive.
    Unsupported,
}

/// Show a notification through the OS notification service.
pub fn show_notification(
    notification: &DesktopNotification,
) -> anyhow::Result<NotificationDelivery> {
    #[cfg(target_os = "macos")]
    {
        macos_notification(notification)
    }
    #[cfg(target_os = "linux")]
    {
        linux_notification(notification)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = notification;
        Ok(NotificationDelivery::Unsupported)
    }
}

#[cfg(target_os = "macos")]
fn macos_notification(notification: &DesktopNotification) -> anyhow::Result<NotificationDelivery> {
    use anyhow::Context;

    // The menu app registers with the notification center under the app's
    // identity, so prefer its helper mode when the CLI runs from the bundle.
    let helper = std::env::current_exe()
        .ok()
        .and_then(|executable| app_bundle_notification_helper(&executable))
        .filter(|path| path.is_file());
    let output = match helper {
        Some(helper) => std::process::Command::new(helper)
            .args([
                "--deliver-model-notification",
                &notification.title,
                &notification.subtitle,
                &notification.body,
            ])
            .output(),
        None => std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv\n display notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv)\nend run",
                "--",
                &notification.title,
                &notification.subtitle,
                &notification.body,
            ])
            .output(),
    }
    .context("run macOS notification delivery")?;
    anyhow::ensure!(
        output.status.success(),
        "macOS notification failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(NotificationDelivery::Delivered)
}

#[cfg(target_os = "linux")]
fn linux_notification(notification: &DesktopNotification) -> anyhow::Result<NotificationDelivery> {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        return Ok(NotificationDelivery::Unsupported);
    }
    let output = match std::process::Command::new("notify-send")
        .arg("--app-name=Codex Mixin")
        .arg(format!(
            "{} · {}",
            notification.title, notification.subtitle
        ))
        .arg(&notification.body)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(NotificationDelivery::Unsupported);
        }
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        output.status.success(),
        "notify-send failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(NotificationDelivery::Delivered)
}

/// Notification helper inside the macOS app bundle that ships this CLI.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_bundle_notification_helper(current_executable: &Path) -> Option<PathBuf> {
    let resources = current_executable.parent()?;
    if resources.file_name()?.to_str()? != "Resources" {
        return None;
    }
    let contents = resources.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    Some(contents.join("MacOS/CodexMixinMenu"))
}

/// Codex desktop apps that read the managed Codex configuration.
pub fn codex_desktop_apps() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["ChatGPT", "Codex"]
    } else {
        &[]
    }
}

/// A running desktop app process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunningDesktopApp {
    pub pid: u32,
    pub started_at: Option<SystemTime>,
}

/// Find the running main process of a desktop app from [`codex_desktop_apps`].
pub fn running_desktop_app(app: &str) -> Option<RunningDesktopApp> {
    #[cfg(target_os = "macos")]
    {
        let pid = macos_app_pid(app)?;
        Some(RunningDesktopApp {
            pid,
            started_at: macos_process_start_time(pid),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        None
    }
}

/// Command a user can run to restart a desktop app manually.
pub fn desktop_app_restart_hint(app: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("osascript -e 'quit app \"{app}\"' && sleep 2 && open -a {app}")
    } else {
        format!("quit and reopen {app}")
    }
}

/// How users make environment variables visible to desktop apps.
pub fn desktop_environment_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "Codex Desktop environment variables need launchctl setenv"
    } else if cfg!(windows) {
        "Codex Desktop reads user environment variables at sign-in; set them with setx and sign in again"
    } else {
        "Codex Desktop inherits environment variables from the desktop session"
    }
}

/// Quit a desktop app, wait for it to exit, and launch it again.
pub fn restart_desktop_app(app: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::time::{Duration, Instant};

        let previous_pid = macos_app_pid(app);
        let quit = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &format!("quit app \"{app}\"")])
            .status()?;
        anyhow::ensure!(quit.success(), "failed to quit {app}");
        match previous_pid {
            Some(pid) => {
                let deadline = Instant::now() + Duration::from_secs(15);
                while super::pid_is_running(pid, app)? {
                    anyhow::ensure!(
                        Instant::now() < deadline,
                        "{app} did not exit within 15s (a window may be blocking quit); restart it manually"
                    );
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
            None => std::thread::sleep(Duration::from_secs(2)),
        }
        let open = std::process::Command::new("/usr/bin/open")
            .args(["-a", app])
            .status()?;
        anyhow::ensure!(open.success(), "failed to reopen {app}");
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("restarting {app} is not supported on this operating system")
    }
}

/// Finds the main process of a desktop app bundle by matching the executable
/// path suffix, which is more reliable than `pgrep -x` for .app bundles.
#[cfg(target_os = "macos")]
fn macos_app_pid(app: &str) -> Option<u32> {
    let output = std::process::Command::new("/bin/ps")
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
fn macos_process_start_time(pid: u32) -> Option<SystemTime> {
    let output = std::process::Command::new("/bin/ps")
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
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_ps_etime(raw: &str) -> Option<std::time::Duration> {
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
    Some(std::time::Duration::from_secs(
        ((days * 24 + hours) * 60 + minutes) * 60 + seconds,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_notification_helper_only_inside_app_resources() {
        assert_eq!(
            app_bundle_notification_helper(Path::new(
                "/Applications/Codex Mixin.app/Contents/Resources/codex-mixin"
            )),
            Some(PathBuf::from(
                "/Applications/Codex Mixin.app/Contents/MacOS/CodexMixinMenu"
            ))
        );
        assert_eq!(
            app_bundle_notification_helper(Path::new("/usr/local/bin/codex-mixin")),
            None
        );
    }

    #[test]
    fn parses_ps_etime_variants() {
        use std::time::Duration;
        assert_eq!(parse_ps_etime("05:20"), Some(Duration::from_secs(320)));
        assert_eq!(parse_ps_etime("01:02:03"), Some(Duration::from_secs(3723)));
        assert_eq!(
            parse_ps_etime("2-01:02:03"),
            Some(Duration::from_secs(2 * 86_400 + 3723))
        );
        assert_eq!(parse_ps_etime(""), None);
        assert_eq!(parse_ps_etime("garbage"), None);
    }
}
