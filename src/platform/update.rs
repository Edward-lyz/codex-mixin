use anyhow::Context;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn release_target() -> anyhow::Result<&'static str> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn target_for(os: &str, arch: &str) -> anyhow::Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        _ => anyhow::bail!("automatic update is not available for {os}/{arch}"),
    }
}

pub fn unpack_release(archive: &Path, destination: &Path) -> anyhow::Result<()> {
    let mut command = std::process::Command::new("tar");
    command
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .arg("--strip-components=1");
    super::prepare_background_command(&mut command);
    let output = command
        .output()
        .context("unpack CLI release with system tar")?;
    anyhow::ensure!(
        output.status.success(),
        "unpack CLI release: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

pub fn replace_executable(target: &Path, downloaded: &Path) -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_targets_cover_supported_desktops_and_servers() {
        for (os, arch, target) in [
            ("linux", "x86_64", "x86_64-unknown-linux-musl"),
            ("linux", "aarch64", "aarch64-unknown-linux-musl"),
            ("macos", "x86_64", "x86_64-apple-darwin"),
            ("macos", "aarch64", "aarch64-apple-darwin"),
            ("windows", "x86_64", "x86_64-pc-windows-msvc"),
        ] {
            assert_eq!(target_for(os, arch).unwrap(), target);
        }
        assert!(target_for("unknown", "x86_64").is_err());
    }
}
