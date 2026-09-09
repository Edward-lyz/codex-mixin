use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilePermissionStatus {
    Private,
    TooOpen(u32),
    Missing,
}

pub fn config_permissions(path: &Path) -> anyhow::Result<FilePermissionStatus> {
    match std::fs::metadata(path) {
        #[cfg(unix)]
        Ok(metadata) => {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode() & 0o777;
            Ok(if mode & 0o077 == 0 {
                FilePermissionStatus::Private
            } else {
                FilePermissionStatus::TooOpen(mode)
            })
        }
        #[cfg(not(unix))]
        Ok(_) => Ok(FilePermissionStatus::Private),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(FilePermissionStatus::Missing)
        }
        Err(error) => Err(error.into()),
    }
}
