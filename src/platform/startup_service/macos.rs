use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::{Context, bail};

use super::definition::{LAUNCHD_LABEL, launchd_program, render_launchd_plist};
use super::{StartupServiceSpec, StartupServiceStatus};

pub(super) const SUPPORTED: bool = true;
const BOOTSTRAP_ATTEMPTS: usize = 10;
const BOOTSTRAP_RETRY_DELAY: Duration = Duration::from_millis(500);

fn agent_path() -> anyhow::Result<PathBuf> {
    Ok(crate::platform::home_dir_required()?
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
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
    let output = launchctl(&["print", &format!("{domain}/{LAUNCHD_LABEL}")])?;
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

fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

pub(super) fn status(spec: &StartupServiceSpec<'_>) -> anyhow::Result<StartupServiceStatus> {
    let path = agent_path()?;
    let installed = path
        .try_exists()
        .with_context(|| format!("inspect {}", path.display()))?;
    let loaded = is_loaded(&domain()?)?;
    let current = installed
        && launchd_program(&read_plist_json(&path)?, spec)?
            .is_some_and(|program| is_executable_file(std::path::Path::new(&program)));
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
        let output = launchctl(&["kickstart", &format!("{domain}/{LAUNCHD_LABEL}")])?;
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
    let output = launchctl(&["bootout", &format!("{domain}/{LAUNCHD_LABEL}")])?;
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::definition::{launchd_program, render_launchd_plist};
    use super::{StartupServiceSpec, read_plist_json};

    #[test]
    fn rendered_launch_agent_round_trips_through_plutil() {
        let directory =
            std::env::temp_dir().join(format!("codex-mixin-plist-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let executable = Path::new("/Applications/Codex Mixin.app/Contents/Resources/codex-mixin");
        let log = Path::new("/Users/me/.codex-mixin/gateway & log.txt");
        let spec = StartupServiceSpec {
            executable,
            log_file: log,
        };
        let path = directory.join("agent.plist");
        std::fs::write(
            &path,
            render_launchd_plist(&spec, Path::new("/Users/me")).unwrap(),
        )
        .unwrap();
        let program = launchd_program(&read_plist_json(&path).unwrap(), &spec).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(program.as_deref(), executable.to_str());
    }
}
