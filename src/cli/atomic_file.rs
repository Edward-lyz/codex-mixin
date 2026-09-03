use std::fs;
use std::io::Write;
use std::path::Path;

pub(super) fn write_atomic_if_changed(path: &Path, contents: &[u8]) -> anyhow::Result<bool> {
    if path.exists() && fs::read(path)? == contents {
        return Ok(false);
    }
    let existing_permissions = if path.exists() {
        Some(fs::metadata(path)?.permissions())
    } else {
        None
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model-catalog.json");
    let temporary_path = path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    fs::write(&temporary_path, contents)?;
    if let Some(permissions) = existing_permissions {
        fs::set_permissions(&temporary_path, permissions)?;
    }
    if let Err(err) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(err.into());
    }
    Ok(true)
}

/// Atomically write a secret file that must stay owner-only (0600).
pub(super) fn write_owner_only(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    if path.exists() && fs::read(path)? == contents {
        return set_owner_only(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("secret");
    let temporary_path = path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let write_result = (|| -> anyhow::Result<()> {
        let mut temporary = options.open(&temporary_path)?;
        temporary.write_all(contents)?;
        fs::rename(&temporary_path, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result?;
    set_owner_only(path)
}

pub(super) fn set_owner_only(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

pub(super) fn ensure_owner_only_dir(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
