use std::fs;
use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, value};

use crate::cli::atomic_file::write_atomic_if_changed;

#[derive(Debug, Eq, PartialEq)]
pub(in crate::cli) struct CodexInstallPaths {
    pub(in crate::cli) config: PathBuf,
    pub(in crate::cli) catalog: PathBuf,
    pub(in crate::cli) models_cache: PathBuf,
}

pub(in crate::cli) struct ManagedConfigLock {
    _file: fs::File,
}

impl ManagedConfigLock {
    pub(in crate::cli) fn acquire(config_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        // A sibling file keeps the lock inode stable while config writes use atomic replacement.
        let lock_path = sibling_path_with_extra_extension(config_path, "codex-mixin.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock_file_exclusive(&file).map_err(|error| {
            anyhow::anyhow!(
                "failed to lock managed Codex config {}: {error}",
                config_path.display()
            )
        })?;
        Ok(Self { _file: file })
    }
}

// Unix keeps the baseline `std::fs::File::lock()` (advisory flock); other
// platforms (Windows) have no flock, so use `fs2`'s cross-platform exclusive
// lock. This keeps Unix/macOS behaviour identical to the pre-Windows baseline.
#[cfg(unix)]
fn lock_file_exclusive(file: &fs::File) -> std::io::Result<()> {
    file.lock()
}

#[cfg(not(unix))]
fn lock_file_exclusive(file: &fs::File) -> std::io::Result<()> {
    use fs2::FileExt;
    file.lock_exclusive()
}

pub(in crate::cli) fn resolve_codex_install_paths(
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
) -> anyhow::Result<CodexInstallPaths> {
    let config = resolve_codex_config_path(config_path)?;
    let codex_home = config
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?
        .to_path_buf();
    let catalog = match catalog_path {
        Some(path) => absolute_path(path)?,
        None => codex_home.join("model-catalogs").join("mixin-models.json"),
    };
    Ok(CodexInstallPaths {
        config,
        catalog,
        models_cache: codex_home.join("models_cache.json"),
    })
}

pub(in crate::cli) fn resolve_codex_config_path(
    config_path: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    absolute_path(config_path.unwrap_or_else(default_codex_config_path))
}

pub(in crate::cli) fn absolute_path(path: PathBuf) -> anyhow::Result<PathBuf> {
    Ok(std::path::absolute(path)?)
}

pub(in crate::cli) fn managed_catalog_path(
    doc: &DocumentMut,
    config_path: &Path,
) -> anyhow::Result<PathBuf> {
    let catalog_path = PathBuf::from(
        doc.get("model_catalog_json")
            .and_then(Item::as_str)
            .ok_or_else(|| anyhow::anyhow!("managed Codex config has no model_catalog_json"))?,
    );
    if catalog_path.is_absolute() {
        absolute_path(catalog_path)
    } else {
        absolute_path(
            config_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?
                .join(catalog_path),
        )
    }
}

pub(in crate::cli) fn read_managed_config_for_install(
    config_path: &Path,
) -> anyhow::Result<String> {
    let raw_config = if config_path.exists() {
        fs::read_to_string(config_path)?
    } else {
        String::new()
    };
    if codex_mixin::clients::codex::document_is_managed(&raw_config) {
        return Ok(raw_config);
    }
    let backup_path = managed_backup_path(config_path);
    let absent_marker_path = managed_absent_marker_path(config_path);
    for restore_path in [backup_path, absent_marker_path] {
        if !restore_path.exists() {
            continue;
        }
        let archived_path = archive_stale_restore_point(config_path)?;
        fs::rename(&restore_path, &archived_path)?;
        eprintln!(
            "codex install: archived stale restore point {} as {}",
            restore_path.display(),
            archived_path.display()
        );
    }
    Ok(raw_config)
}

fn archive_stale_restore_point(config_path: &Path) -> anyhow::Result<PathBuf> {
    for index in 0..1000 {
        let suffix = if index == 0 {
            "codex-mixin.stale".to_owned()
        } else {
            format!("codex-mixin.stale.{index}")
        };
        let candidate = sibling_path_with_extra_extension(config_path, &suffix);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "could not allocate a stale Codex restore point path for {}",
        config_path.display()
    )
}

pub(in crate::cli) fn create_managed_config_restore_point(
    config_path: &Path,
    raw_config: &str,
) -> anyhow::Result<()> {
    if codex_mixin::clients::codex::document_is_managed(raw_config) {
        return Ok(());
    }
    let backup_path = managed_backup_path(config_path);
    let absent_marker_path = managed_absent_marker_path(config_path);
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if config_path.exists() {
        fs::copy(config_path, &backup_path)?;
    } else {
        fs::write(&absent_marker_path, b"")?;
    }
    Ok(())
}

pub(in crate::cli) fn managed_backup_path(config_path: &std::path::Path) -> PathBuf {
    sibling_path_with_extra_extension(config_path, "codex-mixin.backup")
}

pub(in crate::cli) fn managed_absent_marker_path(config_path: &std::path::Path) -> PathBuf {
    sibling_path_with_extra_extension(config_path, "codex-mixin.absent")
}

pub(in crate::cli) fn sibling_path_with_extra_extension(
    config_path: &std::path::Path,
    suffix: &str,
) -> PathBuf {
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    config_path.with_file_name(format!("{file_name}.{suffix}"))
}

pub(in crate::cli) fn default_codex_config_path() -> PathBuf {
    codex_home_path().join("config.toml")
}

pub(in crate::cli) fn codex_home_path() -> PathBuf {
    std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| codex_mixin::platform::home_dir().join(".codex"))
}

pub(in crate::cli) fn sync_installed_codex_client_key(
    config_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let config_path = resolve_codex_config_path(config_path)?;
    codex_mixin::application::client::sync_managed_client_key(
        codex_mixin::gateway_access::GatewayClient::Codex,
        || codex_mixin::clients::codex::is_managed(&config_path),
        |key| codex_mixin::clients::codex::sync_client_key(&config_path, key),
    )?;
    Ok(())
}

pub(in crate::cli) fn sync_managed_codex_gateway_base_url(
    config_path: &Path,
    bind: SocketAddr,
) -> anyhow::Result<bool> {
    let config_path = absolute_path(config_path.to_path_buf())?;
    if !config_path.exists() {
        return Ok(false);
    }
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
    let raw_config = fs::read_to_string(&config_path)?;
    if !codex_mixin::clients::codex::document_is_managed(&raw_config) {
        return Ok(false);
    }
    let mut doc = raw_config.parse::<DocumentMut>()?;
    let provider_id = codex_mixin::clients::codex::managed_provider_id(&doc)?.to_owned();
    let mut changed = false;
    if provider_id == codex_mixin::clients::codex::CUSTOM_ONLY_PROVIDER
        || provider_id == codex_mixin::clients::codex::LEGACY_CUSTOM_ONLY_PROVIDER
    {
        changed |= doc.remove("forced_login_method").is_some();
    }
    let provider = doc
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .and_then(|providers| providers.get_mut(provider_id.as_str()))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| {
            anyhow::anyhow!("managed Codex config has no {provider_id} provider table")
        })?;
    let base_url = format!("http://{bind}/v1");
    if provider.get("base_url").and_then(Item::as_str) != Some(base_url.as_str()) {
        provider["base_url"] = value(base_url);
        changed = true;
    }
    if !changed {
        return Ok(false);
    }
    write_atomic_if_changed(&config_path, doc.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_mixin::clients::codex::MANAGED_HEADER;

    #[test]
    fn managed_header_is_idempotent() {
        let body = "# User settings\nmodel = \"example\" # Keep this\n";
        let expected = format!("{MANAGED_HEADER}\n{body}");
        for count in [0, 1, 20] {
            let raw = format!("{}{body}", format!("{MANAGED_HEADER}\n").repeat(count));
            let mut doc = raw.parse::<DocumentMut>().unwrap();
            for _ in 0..3 {
                let serialized = codex_mixin::clients::codex::serialize(&doc);
                assert_eq!(serialized, expected);
                doc = serialized.parse().unwrap();
            }
        }
    }

    #[test]
    fn managed_header_preserves_data() {
        let raw = format!(
            "# User note\r\n{MANAGED_HEADER}\r\n\r\n# Another note\r\n{MANAGED_HEADER}\r\ninstructions = '''\n{MANAGED_HEADER}\n'''\n"
        );
        let doc = raw.parse::<DocumentMut>().unwrap();
        let serialized = codex_mixin::clients::codex::serialize(&doc);
        let reparsed = serialized.parse::<DocumentMut>().unwrap();
        assert_eq!(
            reparsed["instructions"].as_str(),
            doc["instructions"].as_str()
        );
        assert_eq!(serialized.matches(MANAGED_HEADER).count(), 2);
        assert!(serialized.contains("# User note"));
        assert!(serialized.contains("# Another note"));
        assert_eq!(
            codex_mixin::clients::codex::serialize(&reparsed),
            serialized
        );
    }

    #[test]
    fn unmanaged_config_replaces_stale_restore_point() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("config.toml");
        let backup_path = managed_backup_path(&config_path);
        let current = "model_provider = \"openai\"\n";
        let previous = "model_provider = \"custom\"\n";
        fs::write(&config_path, current).unwrap();
        fs::write(&backup_path, previous).unwrap();

        let raw = read_managed_config_for_install(&config_path).unwrap();

        assert_eq!(raw, current);
        assert!(!backup_path.exists());
        let stale_path = sibling_path_with_extra_extension(&config_path, "codex-mixin.stale");
        assert_eq!(fs::read_to_string(stale_path).unwrap(), previous);

        create_managed_config_restore_point(&config_path, &raw).unwrap();
        assert_eq!(fs::read_to_string(backup_path).unwrap(), current);
    }
}
