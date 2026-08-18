use std::fs;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use anyhow::Context;

use super::service::restart;
pub(super) fn cli_release_target() -> anyhow::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => anyhow::bail!("automatic update is not available for {os}/{arch}"),
    }
}

pub(super) fn release_version_from_redirect(effective_url: &str) -> anyhow::Result<String> {
    let segment = effective_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty() && *segment != "latest")
        .ok_or_else(|| {
            anyhow::anyhow!("GitHub did not redirect to a release; proxy or rate limit response")
        })?;
    let version = segment.trim_start_matches('v');
    ensure_version_chars(version)?;
    Ok(version.to_owned())
}

fn ensure_version_chars(version: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !version.is_empty()
            && version
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '.'
                    || character == '-'),
        "GitHub returned an invalid release version: {version}"
    );
    Ok(())
}

pub(super) fn replace_executable(target: &Path, downloaded: &Path) -> anyhow::Result<()> {
    let parent = target
        .parent()
        .context("current executable has no parent directory")?;
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos();
    let staged = parent.join(format!(".codex-mixin-update-{token}.new"));
    let backup = parent.join(format!(".codex-mixin-update-{token}.old"));
    fs::copy(downloaded, &staged)?;
    #[cfg(unix)]
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    let replace_result: anyhow::Result<()> = (|| -> anyhow::Result<()> {
        fs::rename(target, &backup).with_context(|| {
            format!("failed to move current executable to {}", backup.display())
        })?;
        if let Err(error) = fs::rename(&staged, target) {
            fs::rename(&backup, target).with_context(|| {
                format!(
                    "failed to restore {} after update failure: {error}",
                    target.display()
                )
            })?;
            return Err(error.into());
        }
        let _ = fs::remove_file(&backup);
        Ok(())
    })();
    if replace_result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    replace_result
}

pub(super) async fn run() -> anyhow::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let response = ProcessCommand::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--output",
            "/dev/null",
            "--write-out",
            "%{url_effective}",
            "--max-time",
            "60",
            "https://github.com/Edward-lyz/codex-mixin/releases/latest",
        ])
        .output()
        .context("cannot query GitHub releases; check HTTP_PROXY/HTTPS_PROXY")?;
    anyhow::ensure!(
        response.status.success(),
        "cannot query GitHub releases; curl exited with {}",
        response.status
    );
    let effective_url = String::from_utf8(response.stdout)?.trim().to_owned();
    let latest = release_version_from_redirect(&effective_url)?;
    if latest == current {
        println!("codex-mixin {current} is already up to date.");
        return Ok(());
    }
    let asset = cli_release_target()?;
    let url = format!(
        "https://github.com/Edward-lyz/codex-mixin/releases/download/v{latest}/codex-mixin-cli-{asset}.tar.gz"
    );
    let temp = tempfile::tempdir()?;
    let archive = temp.path().join("codex-mixin.tar.gz");
    println!("Downloading codex-mixin {latest}...");
    let status = ProcessCommand::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--max-time",
            "600",
            "--output",
        ])
        .arg(&archive)
        .arg(&url)
        .status()?;
    anyhow::ensure!(
        status.success(),
        "download failed for {url}; download the asset manually"
    );
    let status = ProcessCommand::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(temp.path())
        .arg("--strip-components=1")
        .status()?;
    anyhow::ensure!(status.success(), "failed to unpack downloaded release");
    let downloaded = temp.path().join("codex-mixin");
    anyhow::ensure!(
        downloaded.is_file(),
        "release archive did not contain codex-mixin"
    );
    anyhow::ensure!(
        fs::metadata(&downloaded)?.len() > 1024 * 1024,
        "downloaded release is unexpectedly small: {}",
        downloaded.display()
    );
    replace_executable(&std::env::current_exe()?, &downloaded)?;
    println!("Updated codex-mixin to {latest}; restarting gateway...");
    restart(None, None, false).await
}
