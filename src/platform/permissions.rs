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
Set-Acl -LiteralPath $env:CODEX_MIXIN_ACL_PATH -AclObject $acl
"#;
    let mut command = Command::new("powershell.exe");
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn private_acl_native_modules() {
        let directory = tempfile::tempdir().unwrap();
        let modules = directory.path().join("modules");
        let security = modules.join("Microsoft.PowerShell.Security").join("99.0.0");
        std::fs::create_dir_all(&security).unwrap();
        std::fs::write(
            security.join("Microsoft.PowerShell.Security.psd1"),
            "@{ RootModule = 'Security.psm1'; ModuleVersion = '99.0.0'; FunctionsToExport = @('Get-Acl', 'Set-Acl') }",
        ).unwrap();
        std::fs::write(
            security.join("Security.psm1"),
            "throw 'Incompatible foreign security module'",
        )
        .unwrap();
        let mut probe = Command::new("powershell.exe");
        probe
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Import-Module Microsoft.PowerShell.Security -ErrorAction Stop",
            ])
            .env("PSModulePath", &modules);
        crate::platform::prepare_background_command(&mut probe);
        let broken = probe.output().unwrap();
        assert!(
            !broken.status.success(),
            "foreign module fixture did not fail"
        );
        assert!(
            String::from_utf8_lossy(&broken.stderr)
                .contains("Incompatible foreign security module")
        );
        // Run the real ACL behavior test in a child instead of mutating the
        // process environment shared by parallel Rust tests.
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "platform::permissions::tests::private_acl_removes_public",
                "--nocapture",
            ])
            .env("PSModulePath", modules);
        crate::platform::prepare_background_command(&mut child);
        let output = child.output().unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
        assert!(
            output.status.success(),
            "ACL test under foreign module path failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn private_acl_removes_public() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("private.txt");
        std::fs::write(&file, b"private").unwrap();
        for (path, is_directory) in [(file.as_path(), false), (directory.path(), true)] {
            let mut grant = Command::new("icacls.exe");
            grant.arg(path).args(["/grant", "*S-1-1-0:(R)"]);
            crate::platform::prepare_background_command(&mut grant);
            assert!(grant.output().unwrap().status.success());
            restrict_windows_acl(path, is_directory).unwrap();
            const CHECK: &str = r#"
$ErrorActionPreference = 'Stop'
$acl = Get-Acl -LiteralPath $env:CODEX_MIXIN_ACL_PATH
$allowed = @([Security.Principal.WindowsIdentity]::GetCurrent().User.Value, 'S-1-5-18', 'S-1-5-32-544')
if (-not $acl.AreAccessRulesProtected) { throw 'ACL inheritance is still enabled' }
foreach ($rule in $acl.Access) {
    $sid = $rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
    if ($sid -notin $allowed) { throw "Unexpected principal $sid" }
}
"#;
            let mut check = Command::new("powershell.exe");
            check
                .args(["-NoProfile", "-NonInteractive", "-Command", CHECK])
                .env_remove("PSModulePath")
                .env("CODEX_MIXIN_ACL_PATH", path);
            crate::platform::prepare_background_command(&mut check);
            let output = check.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
