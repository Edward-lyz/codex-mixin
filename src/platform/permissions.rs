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
    use std::io::Write;
    use std::process::Command;
    use std::sync::OnceLock;

    // icacls restores a complete DACL in one native operation. Unlike /grant,
    // this removes unrelated explicit grants without launching PowerShell on
    // every config read. Never cache ACL application: permissions can change.
    static USER_SID: OnceLock<Result<String, String>> = OnceLock::new();
    let system = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .context("SystemRoot is missing for private Windows ACLs")?
        .join("System32");
    let owner = USER_SID
        .get_or_init(|| {
            let mut command = Command::new(system.join("whoami.exe"));
            command.args(["/user", "/fo", "csv", "/nh"]);
            super::prepare_background_command(&mut command);
            let output = command
                .output()
                .map_err(|error| format!("query Windows user SID: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "query Windows user SID exited with {}",
                    output.status
                ));
            }
            String::from_utf8_lossy(&output.stdout)
                .split(',')
                .map(|value| value.trim().trim_matches('"'))
                .find(|value| {
                    value.starts_with("S-1-")
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || byte == b'-' || byte == b'S')
                })
                .map(str::to_owned)
                .ok_or_else(|| "Windows user query returned no valid SID".to_owned())
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let path = std::path::absolute(path).context("resolve private ACL path")?;
    let parent = path.parent().context("private ACL path has no parent")?;
    let name = path
        .file_name()
        .context("private ACL path has no name")?
        .to_string_lossy();
    anyhow::ensure!(
        !name.contains(['\r', '\n']),
        "private ACL path contains a line break"
    );
    let inheritance = if directory { "OICI" } else { "" };
    let descriptor = format!(
        "{name}\r\nD:P(A;{inheritance};FA;;;{owner})(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;BA)\r\n"
    );
    // The restore format is UTF-16LE with a BOM, including non-ASCII names.
    let mut restore = tempfile::NamedTempFile::new().context("create private ACL restore file")?;
    restore.write_all(&[0xff, 0xfe])?;
    for unit in descriptor.encode_utf16() {
        restore.write_all(&unit.to_le_bytes())?;
    }
    restore.flush()?;
    let mut command = Command::new(system.join("icacls.exe"));
    command
        .arg(parent)
        .arg("/restore")
        .arg(restore.path())
        .arg("/q");
    super::prepare_background_command(&mut command);
    let output = command.output().context("restore private Windows DACL")?;
    anyhow::ensure!(
        output.status.success(),
        "private Windows ACL replacement failed for {}: {} {}",
        path.display(),
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn private_acl_without_sacl_priv() {
        // Hosted Windows runners have admin privileges. Remove the audit
        // privilege before spawning the real test to reproduce a desktop user.
        const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class AuditPrivilege {
    private const uint TokenQueryAndAdjustPrivileges = 0x28;
    private const uint PrivilegeRemoved = 4;
    private const int ErrorNotAllAssigned = 1300;
    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    private struct TokenPrivileges {
        public uint Count;
        public long Luid;
        public uint Attributes;
    }
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool LookupPrivilegeValue(string system, string name, out long luid);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, ref TokenPrivileges privileges, uint length, IntPtr previous, IntPtr returned);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);
    public static void Remove() {
        IntPtr token;
        if (!OpenProcessToken(new IntPtr(-1), TokenQueryAndAdjustPrivileges, out token))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            long luid;
            if (!LookupPrivilegeValue(null, "SeSecurityPrivilege", out luid))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            var privileges = new TokenPrivileges { Count = 1, Luid = luid, Attributes = PrivilegeRemoved };
            if (!AdjustTokenPrivileges(token, false, ref privileges, 0, IntPtr.Zero, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            int error = Marshal.GetLastWin32Error();
            // ERROR_NOT_ALL_ASSIGNED means the ordinary user lacks it already.
            if (error != 0 && error != ErrorNotAllAssigned) throw new Win32Exception(error);
        } finally { CloseHandle(token); }
    }
}
'@
[AuditPrivilege]::Remove()
$privileges = & whoami.exe /priv /fo csv
if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect token privileges' }
if ($privileges -match 'SeSecurityPrivilege') { throw 'Audit privilege still present' }
& $env:CODEX_MIXIN_ACL_TEST_EXE --exact platform::permissions::tests::private_acl_removes_public --nocapture
exit $LASTEXITCODE
"#;
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .env_remove("PSModulePath")
            .env("CODEX_MIXIN_ACL_TEST_EXE", std::env::current_exe().unwrap());
        crate::platform::prepare_background_command(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "ACL test without audit privilege failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
    }

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
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private O'Neil \u{4e2d}\u{6587}");
        std::fs::create_dir(&directory).unwrap();
        let file = directory.join("private \u{6587}\u{4ef6}.txt");
        std::fs::write(&file, b"private").unwrap();
        for (path, is_directory) in [(file.as_path(), false), (directory.as_path(), true)] {
            let mut grant = Command::new("icacls.exe");
            grant
                .arg(path)
                .args(["/grant", "*S-1-1-0:(R)", "*S-1-5-32-545:(R)"]);
            crate::platform::prepare_background_command(&mut grant);
            assert!(grant.output().unwrap().status.success());
            restrict_windows_acl(path, is_directory).unwrap();
            // Repeat on a protected DACL as short-lived CLI queries do.
            restrict_windows_acl(path, is_directory).unwrap();
            const CHECK: &str = r#"
$ErrorActionPreference = 'Stop'
$acl = Get-Acl -LiteralPath $env:CODEX_MIXIN_ACL_PATH
$allowed = @([Security.Principal.WindowsIdentity]::GetCurrent().User.Value, 'S-1-5-18', 'S-1-5-32-544')
if (-not $acl.AreAccessRulesProtected) { throw 'ACL inheritance is still enabled' }
if ($acl.Access.Count -ne 3) { throw 'Expected exactly three private grants' }
foreach ($rule in $acl.Access) {
    $sid = $rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
    if ($sid -notin $allowed) { throw "Unexpected principal $sid" }
    if ($rule.AccessControlType -ne 'Allow' -or $rule.FileSystemRights -ne 'FullControl') { throw "Incorrect private grant for $sid" }
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
