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
    replace_file_atomic(path, contents, existing_permissions, false)?;
    Ok(true)
}

pub fn write_owner_only(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    if path.exists() && fs::read(path)? == contents {
        return set_owner_only(path);
    }
    replace_file_atomic(path, contents, None, true)
}

fn replace_file_atomic(
    path: &Path,
    contents: &[u8],
    permissions: Option<fs::Permissions>,
    owner_only: bool,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(contents)?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    if owner_only {
        crate::platform::restrict_owner_only_file(temporary.path())?;
    }
    temporary.as_file().sync_all()?;
    // TempPath::persist uses replace semantics on Windows and rename on Unix,
    // so the destination is never deleted before the new file is ready.
    temporary.into_temp_path().persist(path)?;
    Ok(())
}

pub fn set_owner_only(path: &Path) -> anyhow::Result<()> {
    crate::platform::restrict_owner_only_file(path)
}

pub fn ensure_owner_only_dir(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    crate::platform::restrict_owner_only_dir(path)
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
