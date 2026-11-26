use std::fs;
use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, value};

use codex_mixin::platform::write_atomic_if_changed;

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
        let lock_path = super::restore::lock_path(config_path);
        codex_mixin::platform::ensure_owner_only_dir(
            lock_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Codex lock has no parent"))?,
        )?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        codex_mixin::platform::lock_exclusive(&file).map_err(|error| {
            anyhow::anyhow!(
                "failed to lock managed Codex config {}: {error}",
                config_path.display()
            )
        })?;
        super::restore::recover(config_path)?;
        Ok(Self { _file: file })
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
    let mut normalized = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    Ok(normalized)
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
    if let Some(record) = super::restore::ManagedCodexRestore::load(config_path)? {
        return record.install_config(&raw_config);
    }
    Ok(raw_config)
}

pub(in crate::cli) fn managed_backup_path(config_path: &std::path::Path) -> PathBuf {
    super::restore::directory(config_path).join("config.toml.backup")
}

pub(in crate::cli) fn managed_absent_marker_path(config_path: &std::path::Path) -> PathBuf {
    super::restore::directory(config_path).join("config.toml.absent")
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
    let base_url = format!("http://{}/v1", codex_mixin::config::client_address(bind));
    if provider.get("base_url").and_then(Item::as_str) != Some(base_url.as_str()) {
        provider["base_url"] = value(&base_url);
        changed = true;
    }
    let official = provider_id == codex_mixin::CODEX_MIXIN_PROVIDER
        && provider.get("supports_websockets").and_then(Item::as_bool) == Some(true);
    if official {
        if doc.get("openai_base_url").and_then(Item::as_str) != Some(base_url.as_str()) {
            doc["openai_base_url"] = value(base_url);
            changed = true;
        }
        if doc.get("model_provider").and_then(Item::as_str)
            != Some(codex_mixin::clients::codex::provider_id(true))
        {
            doc["model_provider"] = value(codex_mixin::clients::codex::provider_id(true));
            changed = true;
        }
    }
    if !changed {
        return Ok(false);
    }
    write_atomic_if_changed(&config_path, doc.to_string().as_bytes())
}
