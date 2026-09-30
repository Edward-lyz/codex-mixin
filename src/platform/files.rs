//! File-system semantics that differ between operating systems: permission
//! bits, executable file names, file locks, and atomic replacement.

use std::fs;
use std::path::Path;

/// File-name suffix of native executables (`.exe` on Windows, empty elsewhere).
pub const EXECUTABLE_SUFFIX: &str = std::env::consts::EXE_SUFFIX;

/// Native executable file name for a program stem, e.g. `codex` -> `codex.exe`.
pub fn executable_file_name(stem: &str) -> String {
    format!("{stem}{EXECUTABLE_SUFFIX}")
}

/// Privacy of a file as far as the OS permission model can express it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerOnlyStatus {
    /// Only the owner can read or write the file.
    Private,
    /// Group or other users have access; carries the POSIX mode bits.
    TooOpen(u32),
    /// The OS has no POSIX mode bits; privacy is governed by inherited ACLs.
    NotApplicable,
}

pub fn owner_only_status(metadata: &fs::Metadata) -> OwnerOnlyStatus {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 == 0 {
            OwnerOnlyStatus::Private
        } else {
            OwnerOnlyStatus::TooOpen(mode)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        OwnerOnlyStatus::NotApplicable
    }
}

/// Whether a file is private to its owner, treating OSes without mode bits
/// as private (their state directory is ACL-restricted instead).
pub fn is_owner_only(path: &Path) -> std::io::Result<bool> {
    Ok(!matches!(
        owner_only_status(&fs::metadata(path)?),
        OwnerOnlyStatus::TooOpen(_)
    ))
}

/// Clear group/other permission bits (mode 0600). No-op where the OS has no
/// POSIX mode bits. Never spawns a process, so it is safe on warm paths.
pub fn set_owner_only_mode(path: &Path) -> std::io::Result<()> {
    set_mode(path, 0o600)
}

/// Directory counterpart of [`set_owner_only_mode`] (mode 0700).
pub fn set_owner_only_dir_mode(path: &Path) -> std::io::Result<()> {
    set_mode(path, 0o700)
}

/// Open-file counterpart of [`set_owner_only_mode`].
pub fn set_owner_only_mode_on(file: &fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

/// Mark a file as runnable by everyone and writable by its owner (0755).
/// Windows decides executability by file extension, so this is a no-op there.
pub fn make_executable(path: &Path) -> std::io::Result<()> {
    set_mode(path, 0o755)
}

/// Owner-only executable (mode 0700) for private helper binaries.
pub fn make_private_executable(path: &Path) -> std::io::Result<()> {
    set_mode(path, 0o700)
}

/// Open-file counterpart of [`make_private_executable`].
pub fn make_private_executable_on(file: &fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// Take an exclusive advisory lock on an open file, blocking until acquired.
pub fn lock_exclusive(file: &fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        file.lock()
    }
    #[cfg(not(unix))]
    {
        fs2::FileExt::lock_exclusive(file)
    }
}

#[cfg(windows)]
static FILE_REPLACE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Atomically move a finished temporary file over `path`. Uses replace
/// semantics on Windows and rename on Unix, so the destination is never
/// deleted before the replacement is ready.
pub fn persist_temp_path(temporary: tempfile::TempPath, path: &Path) -> anyhow::Result<()> {
    // MoveFileEx can reject simultaneous replacements even when every source
    // handle is closed. Writes are a cold path, so serialize only this call.
    #[cfg(windows)]
    let _guard = FILE_REPLACE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    temporary.persist(path)?;
    Ok(())
}

/// Create a symbolic link to a file. Windows requires Developer Mode or an
/// elevated token for this, so callers must handle the error.
pub fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_names_use_the_native_suffix() {
        assert_eq!(
            executable_file_name("codex"),
            format!("codex{}", std::env::consts::EXE_SUFFIX)
        );
    }

    #[test]
    fn owner_only_mode_makes_a_file_private() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        fs::write(&path, b"x").unwrap();
        make_executable(&path).unwrap();
        set_owner_only_mode(&path).unwrap();
        assert!(is_owner_only(&path).unwrap());
        assert_ne!(
            owner_only_status(&fs::metadata(&path).unwrap()),
            OwnerOnlyStatus::TooOpen(0o644)
        );
    }
}
