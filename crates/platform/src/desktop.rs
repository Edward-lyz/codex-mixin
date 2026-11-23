//! Desktop-session services: user notifications and the Codex desktop apps
//! that load the managed configuration.
//!
//! Functions here block on OS tools; async callers use `spawn_blocking`.

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
    #[cfg(windows)]
    {
        windows_notification(notification)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = notification;
        Ok(NotificationDelivery::Unsupported)
    }
}

#[cfg(target_os = "macos")]
fn macos_notification(notification: &DesktopNotification) -> anyhow::Result<NotificationDelivery> {
    use std::process::Command;

    use anyhow::Context;

    let executable = std::env::current_exe().context("locate gateway executable")?;
    let Some(contents) = executable.parent().and_then(std::path::Path::parent) else {
        return Ok(NotificationDelivery::Unsupported);
    };
    if contents.file_name().and_then(|name| name.to_str()) != Some("Contents")
        || contents.parent().and_then(|path| path.extension()) != Some("app".as_ref())
    {
        return Ok(NotificationDelivery::Unsupported);
    }
    let macos = contents.join("MacOS");
    let desktop = macos.join("codex-mixin-desktop");
    // Legacy gateways ran from Contents/Resources and used this helper name.
    let helper = if desktop.is_file() {
        desktop
    } else {
        macos.join("CodexMixinMenu")
    };
    let output = Command::new(helper)
        .args([
            "--deliver-model-notification",
            &notification.title,
            &notification.subtitle,
            &notification.body,
        ])
        .output()
        .context("run the Codex Mixin notification helper")?;
    anyhow::ensure!(
        output.status.success(),
        "macOS notification failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(NotificationDelivery::Delivered)
}

#[cfg(target_os = "linux")]
fn linux_notification(notification: &DesktopNotification) -> anyhow::Result<NotificationDelivery> {
    use anyhow::Context;
    use notify_rust::{Hint, Notification};

    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
        && !std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|path| std::path::Path::new(&path).join("bus").exists())
    {
        return Ok(NotificationDelivery::Unsupported);
    }
    const DESKTOP_ENTRY: &str = "com.edward.codex-mixin";
    let icon = std::env::current_exe()
        .ok()
        .map(|executable| executable.with_file_name("codex-mixin.png"))
        .filter(|icon| icon.is_file())
        .and_then(|icon| url::Url::from_file_path(icon).ok());
    Notification::new()
        .appname(&notification.title)
        .summary(&notification.subtitle)
        .body(&notification.body)
        .icon(icon.as_ref().map_or(DESKTOP_ENTRY, url::Url::as_str))
        .hint(Hint::DesktopEntry(DESKTOP_ENTRY.to_owned()))
        .show()
        .context("deliver notification over the desktop session bus")?;
    Ok(NotificationDelivery::Delivered)
}

#[cfg(windows)]
fn windows_notification(
    notification: &DesktopNotification,
) -> anyhow::Result<NotificationDelivery> {
    use anyhow::Context;
    use tauri_winrt_notification::{IconCrop, Toast};
    use windows_registry::CURRENT_USER;

    const APP_ID: &str = "com.edward.codex-mixin";
    let icon = std::env::current_exe()
        .context("locate gateway executable")?
        .with_file_name("codex-mixin.png");
    if !icon.is_file() {
        return Ok(NotificationDelivery::Unsupported);
    }
    let identity = CURRENT_USER.create(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
    identity.set_string("DisplayName", "Codex Mixin")?;
    identity.set_hstring("IconUri", &icon.as_path().into())?;
    Toast::new(APP_ID)
        .title(&notification.subtitle)
        .text1(&notification.body)
        .icon(&icon, IconCrop::Square, "Codex Mixin")
        .show()
        .context("deliver Windows toast")?;
    Ok(NotificationDelivery::Delivered)
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
