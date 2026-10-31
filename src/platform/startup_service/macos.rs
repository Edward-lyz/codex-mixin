use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use anyhow::{Context, bail};
use wait_timeout::ChildExt;

use super::definition::{launchd_label, launchd_program, render_launchd_plist};
use super::{StartupServiceSpec, StartupServiceStatus};

pub(super) const SUPPORTED: bool = true;
const BOOTSTRAP_ATTEMPTS: usize = 10;
const BOOTSTRAP_RETRY_DELAY: Duration = Duration::from_millis(500);
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_VERSION_OUTPUT_BYTES: u64 = 256;

fn agent_path() -> anyhow::Result<PathBuf> {
    Ok(crate::platform::home_dir_required()?
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", launchd_label()?)))
}

fn domain() -> anyhow::Result<String> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .context("resolve current macOS user ID")?;
    anyhow::ensure!(
        output.status.success(),
        "id -u failed while resolving user ID"
    );
    let uid = String::from_utf8(output.stdout).context("decode current user ID")?;
    Ok(format!("gui/{}", uid.trim()))
}

fn launchctl(args: &[&str]) -> anyhow::Result<Output> {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .context("run launchctl")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

fn is_loaded(domain: &str) -> anyhow::Result<bool> {
    let output = launchctl(&["print", &format!("{domain}/{}", launchd_label()?)])?;
    if output.status.success() {
        return Ok(true);
    }
    // launchctl print exits 113 ("Could not find service") when the job is
    // not loaded in the domain; launchctl messages are not localized.
    if output.status.code() == Some(113)
        || String::from_utf8_lossy(&output.stderr).contains("Could not find service")
    {
        return Ok(false);
    }
    bail!("launchctl print failed: {}", stderr(&output))
}

fn read_plist_json(path: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "--"])
        .arg(path)
        .output()
        .context("run plutil on gateway LaunchAgent")?;
    anyhow::ensure!(
        output.status.success(),
        "gateway LaunchAgent {} is not a valid property list: {}",
        path.display(),
        stderr(&output)
    );
    serde_json::from_slice(&output.stdout).context("decode gateway LaunchAgent")
}

fn is_current_program(path: &std::path::Path) -> anyhow::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    if !std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    {
        return Ok(false);
    }
    // Alternate CLI copies are valid only when they report this version.
    // A file sink avoids blocking on a full stdout pipe before the timeout.
    let output = tempfile::NamedTempFile::new()?;
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(output.as_file().try_clone()?)
        .stderr(Stdio::null())
        .spawn()
        .context("check LaunchAgent CLI version")?;
    let Some(status) = child.wait_timeout(VERSION_CHECK_TIMEOUT)? else {
        child.kill().context("stop timed-out CLI version check")?;
        child.wait().context("reap CLI version check")?;
        return Ok(false);
    };
    let mut version = String::new();
    std::fs::File::open(output.path())?
        .take(MAX_VERSION_OUTPUT_BYTES)
        .read_to_string(&mut version)?;
    Ok(status.success() && version.trim() == format!("codex-mixin {}", env!("CARGO_PKG_VERSION")))
}

pub(super) fn status(spec: &StartupServiceSpec<'_>) -> anyhow::Result<StartupServiceStatus> {
    let path = agent_path()?;
    let installed = path
        .try_exists()
        .with_context(|| format!("inspect {}", path.display()))?;
    let loaded = is_loaded(&domain()?)?;
    let current = if installed {
        match launchd_program(&read_plist_json(&path)?, spec)? {
            Some(program) => is_current_program(std::path::Path::new(&program))?,
            None => false,
        }
    } else {
        false
    };
    Ok(StartupServiceStatus {
        installed,
        current,
        loaded,
    })
}

pub(super) fn install(spec: &StartupServiceSpec<'_>) -> anyhow::Result<()> {
    let home = crate::platform::home_dir_required()?;
    let path = agent_path()?;
    let parent = path.parent().context("LaunchAgent path has no parent")?;
    std::fs::create_dir_all(parent).context("create LaunchAgents directory")?;
    let plist = render_launchd_plist(spec, &home)?;
    let temporary = path.with_extension("plist.tmp");
    std::fs::write(&temporary, plist).context("write gateway LaunchAgent")?;
    std::fs::rename(&temporary, &path).context("replace gateway LaunchAgent")
}

pub(super) fn start() -> anyhow::Result<()> {
    let domain = domain()?;
    if is_loaded(&domain)? {
        let output = launchctl(&["kickstart", &format!("{domain}/{}", launchd_label()?)])?;
        anyhow::ensure!(
            output.status.success(),
            "launchctl kickstart failed: {}",
            stderr(&output)
        );
        return Ok(());
    }
    let path = agent_path()?;
    let path = path
        .to_str()
        .context("LaunchAgent path is not valid UTF-8")?;
    let mut last_error = String::new();
    for attempt in 0..BOOTSTRAP_ATTEMPTS {
        let output = launchctl(&["bootstrap", &domain, path])?;
        if output.status.success() {
            return Ok(());
        }
        last_error = stderr(&output);
        // Exit 5 (EIO) is transient while launchd still tears down a job
        // with the same label.
        if output.status.code() != Some(5) {
            break;
        }
        if attempt + 1 < BOOTSTRAP_ATTEMPTS {
            std::thread::sleep(BOOTSTRAP_RETRY_DELAY);
        }
    }
    bail!("launchctl bootstrap failed: {last_error}")
}

pub(super) fn stop() -> anyhow::Result<()> {
    let domain = domain()?;
    if !is_loaded(&domain)? {
        return Ok(());
    }
    let output = launchctl(&["bootout", &format!("{domain}/{}", launchd_label()?)])?;
    if output.status.success() || !is_loaded(&domain)? {
        return Ok(());
    }
    bail!("launchctl bootout failed: {}", stderr(&output))
}

pub(super) fn remove() -> anyhow::Result<()> {
    stop()?;
    let path = agent_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}
