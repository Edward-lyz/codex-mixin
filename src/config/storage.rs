use super::StoredGatewayConfig;
use super::migration::parse_stored_config;
use crate::config::ensure_config_version;
use crate::fusion::{validate_fusion_model_references, validate_fusion_profiles};
use crate::provider::ProviderRegistry;
use anyhow::{Context, anyhow};
use base64::Engine;
use fs2::FileExt;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
pub fn ensure_compaction_secret() -> anyhow::Result<String> {
    let mut secret = None;
    mutate_stored_config(|config| {
        if config.compaction_secret.is_none() {
            let mut bytes = [0_u8; 32];
            ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
                .map_err(|_| anyhow!("generate compaction secret"))?;
            config.compaction_secret =
                Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes));
        }
        secret = config.compaction_secret.clone();
        Ok(())
    })?;
    secret.ok_or_else(|| anyhow!("compaction secret was not persisted"))
}

pub fn ensure_gateway_client_key(
    client: crate::gateway_access::GatewayClient,
) -> anyhow::Result<String> {
    let mut client_key = None;
    mutate_stored_config(|config| {
        let stored_key = config.gateway_client_keys.get_mut(client);
        if stored_key.is_none() {
            *stored_key = Some(crate::gateway_access::generate_client_key(client)?);
        }
        client_key.clone_from(stored_key);
        Ok(())
    })?;
    client_key.ok_or_else(|| anyhow!("gateway client key was not persisted"))
}

pub fn gateway_client_key_exists(
    client: crate::gateway_access::GatewayClient,
) -> anyhow::Result<bool> {
    Ok(load_stored_config()?
        .and_then(|config| config.gateway_client_keys.get(client).map(str::to_owned))
        .is_some())
}

pub fn revoke_gateway_client_key(
    client: crate::gateway_access::GatewayClient,
) -> anyhow::Result<()> {
    mutate_stored_config(|config| {
        *config.gateway_client_keys.get_mut(client) = None;
        Ok(())
    })
}
pub fn stored_config_path() -> PathBuf {
    if let Some(path) = env::var("CODEX_GATEWAY_CONFIG")
        .ok()
        .filter(|path| !path.is_empty())
    {
        return PathBuf::from(path);
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join(".codex-mixin").join("config.json")
}
pub fn load_stored_config() -> anyhow::Result<Option<StoredGatewayConfig>> {
    load_stored_config_from_path(&stored_config_path())
}
pub fn load_stored_config_from_path(
    path: &std::path::Path,
) -> anyhow::Result<Option<StoredGatewayConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let parsed = parse_stored_config(&raw)
        .map_err(|error| anyhow!("parse {}: {error:#}", path.display()))?;
    Ok(Some(parsed))
}
pub fn save_stored_config(config: &StoredGatewayConfig) -> anyhow::Result<PathBuf> {
    let path = stored_config_path();
    save_stored_config_to_path(&path, config)?;
    Ok(path)
}
pub fn mutate_stored_config<T>(
    mutation: impl FnOnce(&mut StoredGatewayConfig) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    mutate_stored_config_at_path(&stored_config_path(), mutation)
}
pub fn mutate_stored_config_at_path<T>(
    path: &std::path::Path,
    mutation: impl FnOnce(&mut StoredGatewayConfig) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _lock = lock_stored_config(path)?;
    let mut config = load_stored_config_from_path(path)?.unwrap_or_default();
    let result = mutation(&mut config)?;
    backup_legacy_config_if_needed(path)?;
    save_stored_config_to_path_unlocked(path, &config)?;
    Ok(result)
}
fn backup_legacy_config_if_needed(path: &std::path::Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let document: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    if document.get("config_version").is_some() {
        return Ok(());
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid config filename: {}", path.display()))?;
    let backup_path = path.with_file_name(format!("{file_name}.v1.backup"));
    if backup_path.exists() {
        return Ok(());
    }
    let mut backup = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&backup_path)
        .with_context(|| format!("create legacy config backup {}", backup_path.display()))?;
    set_private_file_permissions(&backup)?;
    backup
        .write_all(raw.as_bytes())
        .with_context(|| format!("write {}", backup_path.display()))?;
    backup
        .sync_all()
        .with_context(|| format!("sync {}", backup_path.display()))?;
    Ok(())
}
pub fn save_stored_config_to_path(
    path: &std::path::Path,
    config: &StoredGatewayConfig,
) -> anyhow::Result<()> {
    let _lock = lock_stored_config(path)?;
    save_stored_config_to_path_unlocked(path, config)
}
fn save_stored_config_to_path_unlocked(
    path: &std::path::Path,
    config: &StoredGatewayConfig,
) -> anyhow::Result<()> {
    ensure_config_version(config.config_version)?;
    let providers = ProviderRegistry::new(config.providers.clone())?;
    validate_fusion_profiles(&config.fusion_profiles)?;
    validate_fusion_model_references(&config.fusion_profiles, &providers)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        set_private_dir_permissions(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid config filename: {}", path.display()))?;
    let temporary_path =
        path.with_file_name(format!("{file_name}.tmp.{}", uuid::Uuid::new_v4().simple()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary_path)
        .with_context(|| format!("open {}", temporary_path.display()))?;
    set_private_file_permissions(&file)?;
    let content = serde_json::to_vec_pretty(config)?;
    file.write_all(&content)
        .with_context(|| format!("write {}", temporary_path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("write {}", temporary_path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", temporary_path.display()))?;
    drop(file);
    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error).with_context(|| format!("replace {}", path.display()));
    }
    Ok(())
}
fn lock_stored_config(path: &std::path::Path) -> anyhow::Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        set_private_dir_permissions(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid config filename: {}", path.display()))?;
    let lock_path = path.with_file_name(format!("{file_name}.lock"));
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;
    set_private_file_permissions(&lock)?;
    FileExt::lock_exclusive(&lock).with_context(|| format!("lock {}", lock_path.display()))?;
    Ok(lock)
}
pub fn delete_stored_config() -> anyhow::Result<bool> {
    let path = stored_config_path();
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(true)
}
fn set_private_dir_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 700 {}", path.display()))?;
    }
    Ok(())
}
fn set_private_file_permissions(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
