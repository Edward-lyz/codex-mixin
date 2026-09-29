use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, bail};

const SERVICE_LABEL: &str = "local.codex-mixin.service";
const THROTTLE_INTERVAL_SECONDS: u8 = 10;

fn agent_path() -> anyhow::Result<PathBuf> {
    Ok(crate::platform::home_dir_required()?
        .join("Library/LaunchAgents")
        .join(format!("{SERVICE_LABEL}.plist")))
}

fn service_target() -> anyhow::Result<String> {
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .context("resolve current macOS user ID")?;
    anyhow::ensure!(uid.status.success(), "id -u failed while resolving user ID");
    let uid = String::from_utf8(uid.stdout).context("decode current user ID")?;
    Ok(format!("gui/{}", uid.trim()))
}

fn run_launchctl(args: &[&str]) -> anyhow::Result<Output> {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .context("run launchctl")
}

fn is_loaded() -> anyhow::Result<bool> {
    let target = format!("{}/{}", service_target()?, SERVICE_LABEL);
    let output = run_launchctl(&["print", &target])?;
    Ok(output.status.success())
}

pub(super) fn is_installed() -> bool {
    agent_path().is_ok_and(|path| path.is_file())
}

pub(super) fn is_enabled() -> bool {
    agent_path()
        .and_then(|path| std::fs::read_to_string(path).context("read gateway LaunchAgent"))
        .is_ok_and(|plist| plist.contains("<key>RunAtLoad</key>\n  <true/>"))
}

pub(super) fn is_running() -> anyhow::Result<bool> {
    is_loaded()
}

pub(super) fn needs_update(executable: &Path, log_file: &Path) -> anyhow::Result<bool> {
    let path = agent_path()?;
    if !path.is_file() {
        return Ok(true);
    }
    let plist = std::fs::read_to_string(path).context("read gateway LaunchAgent")?;
    let arguments = format!(
        "<string>{}</string>\n    <string>start</string>\n    <string>--log-file</string>\n    <string>{}</string>",
        xml_escape(&executable.display().to_string()),
        xml_escape(&log_file.display().to_string())
    );
    Ok(!plist.contains(&arguments)
        || !plist.contains("<key>RunAtLoad</key>\n  <true/>")
        || !plist.contains("<key>SuccessfulExit</key>\n    <false/>")
        || !plist.contains(&format!(
            "<key>ThrottleInterval</key>\n  <integer>{THROTTLE_INTERVAL_SECONDS}</integer>"
        ))
        || !plist.contains("<key>ProcessType</key>\n  <string>Background</string>"))
}

pub(super) fn install(executable: &Path, log_file: &Path) -> anyhow::Result<()> {
    let path = agent_path()?;
    let parent = path.parent().context("LaunchAgent path has no parent")?;
    std::fs::create_dir_all(parent).context("create LaunchAgents directory")?;
    let state_dir = crate::platform::home_dir_required()?.join(".codex-mixin");
    std::fs::create_dir_all(&state_dir).context("create gateway state directory")?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{SERVICE_LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>start</string>\n    <string>--log-file</string>\n    <string>{}</string>\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n  <key>ThrottleInterval</key>\n  <integer>{THROTTLE_INTERVAL_SECONDS}</integer>\n  <key>ProcessType</key>\n  <string>Background</string>\n  <key>StandardOutPath</key>\n  <string>/dev/null</string>\n  <key>StandardErrorPath</key>\n  <string>/dev/null</string>\n  <key>WorkingDirectory</key>\n  <string>{}</string>\n</dict>\n</plist>\n",
        xml_escape(&executable.display().to_string()),
        xml_escape(&log_file.display().to_string()),
        xml_escape(&crate::platform::home_dir_required()?.display().to_string())
    );
    std::fs::write(path, plist).context("write gateway LaunchAgent")
}

pub(super) fn start() -> anyhow::Result<()> {
    if is_loaded()? {
        return Ok(());
    }
    let target = service_target()?;
    let path = agent_path()?;
    let output = run_launchctl(&["bootstrap", &target, &path.to_string_lossy()])?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    if message.contains("Input/output error") {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let retry = run_launchctl(&["bootstrap", &target, &path.to_string_lossy()])?;
        anyhow::ensure!(
            retry.status.success(),
            "launchctl bootstrap failed: {}",
            String::from_utf8_lossy(&retry.stderr).trim()
        );
        return Ok(());
    }
    bail!("launchctl bootstrap failed: {}", message.trim())
}

pub(super) fn stop() -> anyhow::Result<()> {
    let target = format!("{}/{}", service_target()?, SERVICE_LABEL);
    let output = run_launchctl(&["bootout", &target])?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    if message.contains("No such process") || message.contains("Could not find service") {
        return Ok(());
    }
    bail!("launchctl bootout failed: {}", message.trim())
}

pub(super) fn remove() -> anyhow::Result<()> {
    stop()?;
    let path = agent_path()?;
    if path.exists() {
        std::fs::remove_file(path).context("remove gateway LaunchAgent")?;
    }
    Ok(())
}

pub(super) fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let path = agent_path()?;
    let plist = std::fs::read_to_string(&path).context("read gateway LaunchAgent")?;
    let (from, to) = if enabled {
        (
            "<key>RunAtLoad</key>\n  <false/>",
            "<key>RunAtLoad</key>\n  <true/>",
        )
    } else {
        (
            "<key>RunAtLoad</key>\n  <true/>",
            "<key>RunAtLoad</key>\n  <false/>",
        )
    };
    anyhow::ensure!(
        plist.contains(from) || plist.contains(to),
        "gateway LaunchAgent has an invalid RunAtLoad setting"
    );
    if !plist.contains(to) {
        std::fs::write(path, plist.replace(from, to))
            .context("update gateway LaunchAgent autostart")?;
    }
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::xml_escape;

    #[test]
    fn escapes_launch_agent_xml_values() {
        assert_eq!(xml_escape("a&b<'\""), "a&amp;b&lt;&apos;&quot;");
    }
}
