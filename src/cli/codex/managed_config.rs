use std::fs;
use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, value};

use codex_mixin::CODEX_MIXIN_PROVIDER;

use crate::cli::atomic_file::write_atomic_if_changed;

pub(in crate::cli) const MANAGED_CONFIG_MARKER: &str = "codex-mixin managed config";
pub(in crate::cli) const MANAGED_CONFIG_HEADER: &str = "# codex-mixin managed config. Run `codex-mixin uninstall-codex` to restore the previous config.";

#[derive(Debug, Eq, PartialEq)]
pub(in crate::cli) struct CodexInstallPaths {
    pub(in crate::cli) config: PathBuf,
    pub(in crate::cli) catalog: PathBuf,
    pub(in crate::cli) models_cache: PathBuf,
}

pub(in crate::cli) struct ManagedConfigLock {
    #[cfg(unix)]
    _file: fs::File,
}

impl ManagedConfigLock {
    pub(in crate::cli) fn acquire(config_path: &Path) -> anyhow::Result<Self> {
        #[cfg(not(unix))]
        {
            let _ = config_path;
            anyhow::bail!("managed Codex config locking requires Unix flock support");
        }

        #[cfg(unix)]
        {
            if let Some(parent) = config_path.parent() {
                fs::create_dir_all(parent)?;
            }
            // A sibling file keeps the lock inode stable while config writes use atomic rename.
            let lock_path = sibling_path_with_extra_extension(config_path, "codex-mixin.lock");
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)?;
            file.lock().map_err(|error| {
                anyhow::anyhow!(
                    "failed to lock managed Codex config {}: {error}",
                    config_path.display()
                )
            })?;
            Ok(Self { _file: file })
        }
    }
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
    if is_managed_config(&raw_config) {
        return Ok(raw_config);
    }
    let backup_path = managed_backup_path(config_path);
    let absent_marker_path = managed_absent_marker_path(config_path);
    if backup_path.exists() || absent_marker_path.exists() {
        anyhow::bail!(
            "existing codex-mixin restore point found but current config is not managed: {} or {}",
            backup_path.display(),
            absent_marker_path.display()
        );
    }
    Ok(raw_config)
}

pub(in crate::cli) fn create_managed_config_restore_point(
    config_path: &Path,
    raw_config: &str,
) -> anyhow::Result<()> {
    if is_managed_config(raw_config) {
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

pub(in crate::cli) fn is_managed_config(raw_config: &str) -> bool {
    raw_config.contains(MANAGED_CONFIG_MARKER)
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
    std::env::var("CODEX_HOME").ok().map_or_else(
        || {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".codex")
        },
        PathBuf::from,
    )
}

pub(in crate::cli) fn upsert_codex_config(
    doc: &mut DocumentMut,
    default_model: Option<&str>,
    catalog_path: &std::path::Path,
    base_url: &str,
    web_search: &str,
    env_key: Option<&str>,
    codex_oauth_proxy: bool,
) -> anyhow::Result<()> {
    doc["model_catalog_json"] = value(catalog_path.to_string_lossy().to_string());
    doc["model_provider"] = value(CODEX_MIXIN_PROVIDER);
    doc["web_search"] = value(web_search);
    if let Some(model) = default_model {
        doc["model"] = value(model);
    }

    if !doc
        .get("model_providers")
        .is_some_and(|item| item.is_table())
    {
        doc["model_providers"] = Item::Table(Table::new());
    }
    let providers = doc["model_providers"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("model_providers must be a TOML table"))?;
    let mut provider_table = Table::new();
    provider_table["name"] = value("Codex Mixin");
    provider_table["base_url"] = value(base_url);
    provider_table["wire_api"] = value("responses");
    if codex_oauth_proxy {
        provider_table["requires_openai_auth"] = value(true);
        provider_table["supports_websockets"] = value(true);
        provider_table.remove("env_key");
    } else {
        provider_table.remove("requires_openai_auth");
        provider_table.remove("supports_websockets");
        if let Some(env_key) = env_key {
            provider_table["env_key"] = value(env_key);
        } else {
            provider_table.remove("env_key");
        }
    }
    providers.insert(CODEX_MIXIN_PROVIDER, Item::Table(provider_table));
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
    if !is_managed_config(&raw_config) {
        return Ok(false);
    }
    let mut doc = raw_config.parse::<DocumentMut>()?;
    let provider = doc
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .and_then(|providers| providers.get_mut(CODEX_MIXIN_PROVIDER))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("managed Codex config has no codex-mixin provider"))?;
    let base_url = format!("http://{bind}/v1");
    if provider.get("base_url").and_then(Item::as_str) == Some(base_url.as_str()) {
        return Ok(false);
    }
    provider["base_url"] = value(base_url);
    write_atomic_if_changed(&config_path, doc.to_string().as_bytes())
}
