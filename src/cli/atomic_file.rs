use std::fs;
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
