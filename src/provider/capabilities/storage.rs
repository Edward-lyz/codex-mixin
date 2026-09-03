use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use fs2::FileExt;

use crate::config::stored_config_path;

use super::types::{CAPABILITY_FILE_VERSION, CapabilityFile};

pub(super) fn default_capability_path() -> PathBuf {
    stored_config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("provider-capabilities.json")
}

pub(super) fn load_file(path: &Path) -> anyhow::Result<CapabilityFile> {
    if !path.exists() {
        return Ok(CapabilityFile {
            version: CAPABILITY_FILE_VERSION,
            ..CapabilityFile::default()
        });
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let file: CapabilityFile =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    if file.version != CAPABILITY_FILE_VERSION {
        return Ok(CapabilityFile {
            version: CAPABILITY_FILE_VERSION,
            ..CapabilityFile::default()
        });
    }
    Ok(file)
}

pub(super) fn update_file(
    path: &Path,
    update: impl FnOnce(&mut CapabilityFile) -> anyhow::Result<()>,
) -> anyhow::Result<CapabilityFile> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let lock_path = path.with_extension("json.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("lock {}", lock_path.display()))?;

    let mut file = load_file(path)?;
    update(&mut file)?;
    let temporary_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let result = (|| {
        let mut temporary = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .with_context(|| format!("open {}", temporary_path.display()))?;
        serde_json::to_writer_pretty(&mut temporary, &file)
            .with_context(|| format!("serialize {}", path.display()))?;
        temporary.write_all(b"\n")?;
        temporary.sync_all()?;
        fs::rename(&temporary_path, path).with_context(|| format!("replace {}", path.display()))?;
        Ok(file.clone())
    })();
    let _ = fs::remove_file(&temporary_path);
    let _ = FileExt::unlock(&lock);
    result
}

pub(super) fn unix_milliseconds() -> anyhow::Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}
