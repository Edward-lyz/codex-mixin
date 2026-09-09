use std::fs;
use std::io::Write;
use std::path::Path;

pub fn write_atomic_if_changed(path: &Path, contents: &[u8]) -> anyhow::Result<bool> {
    if path.exists() && fs::read(path)? == contents {
        return Ok(false);
    }
    let existing_permissions = path
        .exists()
        .then(|| fs::metadata(path))
        .transpose()?
        .map(|metadata| metadata.permissions());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let temporary_path = path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    fs::write(&temporary_path, contents)?;
    if let Some(permissions) = existing_permissions {
        fs::set_permissions(&temporary_path, permissions)?;
    }
    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error.into());
    }
    Ok(true)
}

pub fn write_owner_only(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
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
    let result = (|| -> anyhow::Result<()> {
        let mut temporary = options.open(&temporary_path)?;
        temporary.write_all(contents)?;
        fs::rename(&temporary_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result?;
    set_owner_only(path)
}

pub fn set_owner_only(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

pub fn ensure_owner_only_dir(path: &Path) -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.json");
        assert!(write_atomic_if_changed(&path, b"one").unwrap());
        assert!(!write_atomic_if_changed(&path, b"one").unwrap());
        assert!(write_atomic_if_changed(&path, b"two").unwrap());
        assert_eq!(fs::read(path).unwrap(), b"two");
    }
}
