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
    use windows_permissions::constants::{SeObjectType, SecurityInformation};
    use windows_permissions::{LocalBox, SecurityDescriptor, utilities, wrappers};

    // A fresh DACL removes explicit grants too; disabling inheritance alone
    // leaves an existing Everyone/Users rule able to read private credentials.
    // Configuration reads call this repeatedly, so use native APIs rather than
    // starting PowerShell for every directory and lock file on the query path.
    let owner = utilities::current_process_sid().context("get current Windows user SID")?;
    let inheritance = if directory { "OICI" } else { "" };
    let descriptor: LocalBox<SecurityDescriptor> = format!(
        "D:P(A;{inheritance};FA;;;{owner})(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;BA)"
    )
    .parse()
    .context("build private Windows ACL")?;
    let dacl = descriptor
        .dacl()
        .context("private Windows ACL is missing")?;
    // Persist only the DACL, leaving the owner and SACL untouched. Protect it
    // from inherited grants, including when repairing an existing file.
    wrappers::SetNamedSecurityInfo(
        path,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
        None,
        None,
        Some(dacl),
        None,
    )
    .with_context(|| format!("replace private Windows ACL for {}", path.display()))
}
