use std::fs;
use std::io::Write;
use std::path::Path;
#[cfg(windows)]
use std::sync::Mutex;

#[cfg(windows)]
static FILE_REPLACE_LOCK: Mutex<()> = Mutex::new(());

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
    persist_temp_path(temporary.into_temp_path(), path)?;
    Ok(())
}

#[cfg(windows)]
fn persist_temp_path(temporary: tempfile::TempPath, path: &Path) -> anyhow::Result<()> {
    // MoveFileEx can reject simultaneous replacements even when every source
    // handle is closed. Writes are a cold path, so serialize only this syscall.
    let _guard = FILE_REPLACE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    temporary.persist(path)?;
    Ok(())
}

#[cfg(not(windows))]
fn persist_temp_path(temporary: tempfile::TempPath, path: &Path) -> anyhow::Result<()> {
    temporary.persist(path)?;
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
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn atomic_write_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.json");
        assert!(write_atomic_if_changed(&path, b"one").unwrap());
        assert!(!write_atomic_if_changed(&path, b"one").unwrap());
        assert!(write_atomic_if_changed(&path, b"two").unwrap());
        assert_eq!(fs::read(path).unwrap(), b"two");
    }

    #[test]
    fn concurrent_atomic_writes_use_distinct_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = Arc::new(directory.path().join("client.json"));
        let writers = 8;
        let barrier = Arc::new(Barrier::new(writers));
        let threads = (0..writers)
            .map(|index| {
                let path = Arc::clone(&path);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let contents = vec![b'0' + index as u8; 64 * 1024];
                    barrier.wait();
                    write_atomic_if_changed(&path, &contents).unwrap();
                    contents
                })
            })
            .collect::<Vec<_>>();
        let candidates = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();

        let written = fs::read(path.as_ref()).unwrap();
        assert!(candidates.contains(&written));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
