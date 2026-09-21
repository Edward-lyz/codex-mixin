use std::path::Path;

pub fn restrict_owner_only_file(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(windows)]
    restrict_windows_acl(path, false)?;
    Ok(())
}

pub fn restrict_owner_only_dir(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions)?;
    }
    #[cfg(windows)]
    restrict_windows_acl(path, true)?;
    Ok(())
}

#[cfg(windows)]
fn restrict_windows_acl(path: &Path, directory: bool) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::process::{Command, Stdio};

    let mut identity = Command::new("whoami.exe");
    identity.args(["/user", "/fo", "csv", "/nh"]);
    super::prepare_background_command(&mut identity);
    let output = identity
        .output()
        .context("resolve current Windows user SID")?;
    anyhow::ensure!(
        output.status.success(),
        "whoami failed while resolving user SID"
    );
    let line = String::from_utf8(output.stdout).context("decode current Windows user SID")?;
    let sid = line
        .trim()
        .trim_matches('"')
        .rsplit_once("\",\"")
        .map(|(_, sid)| sid.trim_matches('"'))
        .filter(|sid| sid.starts_with("S-1-"))
        .context("whoami returned an invalid Windows user SID")?;
    let inheritance = if directory { "(OI)(CI)F" } else { "F" };
    let mut command = Command::new("icacls.exe");
    command
        .arg(path)
        .args([
            "/inheritance:r",
            "/grant:r",
            &format!("*{sid}:{inheritance}"),
            &format!("*S-1-5-18:{inheritance}"),
            &format!("*S-1-5-32-544:{inheritance}"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    super::prepare_background_command(&mut command);
    let output = command.output().context("apply private Windows ACL")?;
    anyhow::ensure!(
        output.status.success(),
        "icacls failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
