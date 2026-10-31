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
    // A fresh DACL removes explicit grants too; disabling inheritance alone
    // leaves an existing Everyone/Users rule able to read private credentials.
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$owner = [Security.Principal.WindowsIdentity]::GetCurrent().User
$directory = $env:CODEX_MIXIN_ACL_DIRECTORY -eq 'true'
if ($directory) {
    $acl = [System.Security.AccessControl.DirectorySecurity]::new()
    $inheritance = [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'
} else {
    $acl = [System.Security.AccessControl.FileSecurity]::new()
    $inheritance = [Security.AccessControl.InheritanceFlags]::None
}
$acl.SetAccessRuleProtection($true, $false)
foreach ($value in @($owner.Value, 'S-1-5-18', 'S-1-5-32-544')) {
    $sid = [System.Security.Principal.SecurityIdentifier]::new($value)
    $rule = [System.Security.AccessControl.FileSystemAccessRule]::new($sid, [Security.AccessControl.FileSystemRights]::FullControl, $inheritance, [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
    $acl.AddAccessRule($rule)
}
# Set-Acl and icacls /restore can require SeSecurityPrivilege
# for the untouched SACL. Persist only the DACL changes made above.
if ($directory) {
    ([System.IO.DirectoryInfo]::new($env:CODEX_MIXIN_ACL_PATH)).SetAccessControl($acl)
} else {
    ([System.IO.FileInfo]::new($env:CODEX_MIXIN_ACL_PATH)).SetAccessControl($acl)
}
"#;
    let mut command = Command::new(super::system_powershell()?);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ])
        // PowerShell 7's inherited module path can select incompatible modules
        // in Windows PowerShell 5.1. Let the child build its native module path.
        .env_remove("PSModulePath")
        .env("CODEX_MIXIN_ACL_PATH", path)
        .env(
            "CODEX_MIXIN_ACL_DIRECTORY",
            if directory { "true" } else { "false" },
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    super::prepare_background_command(&mut command);
    let output = command.output().context("replace private Windows ACL")?;
    anyhow::ensure!(
        output.status.success(),
        "private Windows ACL replacement failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
