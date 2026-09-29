use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilePermissionStatus {
    Private,
    TooOpen(u32),
    Missing,
}

pub fn config_permissions(path: &Path) -> anyhow::Result<FilePermissionStatus> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(match crate::platform::owner_only_status(&metadata) {
            crate::platform::OwnerOnlyStatus::TooOpen(mode) => FilePermissionStatus::TooOpen(mode),
            crate::platform::OwnerOnlyStatus::Private
            | crate::platform::OwnerOnlyStatus::NotApplicable => FilePermissionStatus::Private,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(FilePermissionStatus::Missing)
        }
        Err(error) => Err(error.into()),
    }
}
