use super::StoredGatewayConfig;
use super::migration::parse_stored_config;
use crate::config::ensure_config_version;
use crate::fusion::{validate_fusion_model_references, validate_fusion_profiles};
use crate::provider::ProviderRegistry;
use anyhow::{Context, anyhow};
use base64::Engine;
use fs2::FileExt;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

const CONFIG_ENCRYPTION: &str = "aes-256-gcm";
const CONFIG_KEY_BYTES: usize = 32;
const CONFIG_NONCE_BYTES: usize = 12;

#[derive(serde::Deserialize, serde::Serialize)]
struct EncryptedConfig {
    encryption: String,
    nonce: String,
    ciphertext: String,
}
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
    let _lock = lock_stored_config(path)?;
    load_stored_config_from_path_unlocked(path, true)
}

fn load_stored_config_from_path_unlocked(
    path: &std::path::Path,
    migrate_plaintext: bool,
) -> anyhow::Result<Option<StoredGatewayConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let (plaintext, encrypted) = decrypt_config_if_needed(path, &raw)?;
    let text = std::str::from_utf8(&plaintext)
        .with_context(|| format!("decode {} as UTF-8", path.display()))?;
    let parsed = parse_stored_config(text)
        .map_err(|error| anyhow!("parse {}: {error:#}", path.display()))?;
    if migrate_plaintext && !encrypted {
        backup_legacy_config_if_needed(path)?;
        save_stored_config_to_path_unlocked(path, &parsed)?;
    }
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
    let mut config = load_stored_config_from_path_unlocked(path, false)?.unwrap_or_default();
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
    if document.get("config_version").is_some() || document.get("encryption").is_some() {
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
    let encrypted = encrypt_config(path, raw.as_bytes())?;
    backup
        .write_all(&encrypted)
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
    let plaintext = serde_json::to_vec_pretty(config)?;
    let content = encrypt_config(path, &plaintext)?;
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

pub fn export_stored_config(path: &std::path::Path) -> anyhow::Result<()> {
    let stored_path = std::path::absolute(stored_config_path())?;
    anyhow::ensure!(
        path != stored_path && path != config_key_path(&stored_path)?,
        "plaintext export cannot replace the encrypted config or its key"
    );
    let config =
        load_stored_config()?.ok_or_else(|| anyhow!("provider configuration is missing"))?;
    let content = serde_json::to_vec_pretty(&config)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open plaintext config export {}", path.display()))?;
    set_private_file_permissions(&file)?;
    file.write_all(&content)
        .with_context(|| format!("write plaintext config export {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("write plaintext config export {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync plaintext config export {}", path.display()))
}

fn config_key_path(path: &std::path::Path) -> anyhow::Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid config filename: {}", path.display()))?;
    Ok(path.with_file_name(format!("{file_name}.key")))
}

fn load_or_create_config_key(path: &std::path::Path) -> anyhow::Result<[u8; CONFIG_KEY_BYTES]> {
    let key_path = config_key_path(path)?;
    if key_path.exists() {
        return load_config_key(&key_path);
    }
    let mut key = [0_u8; CONFIG_KEY_BYTES];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut key)
        .map_err(|_| anyhow!("generate config encryption key"))?;
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&key_path)
        .with_context(|| format!("create config encryption key {}", key_path.display()))?;
    set_private_file_permissions(&file)?;
    file.write_all(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(key)
            .as_bytes(),
    )
    .with_context(|| format!("write config encryption key {}", key_path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync config encryption key {}", key_path.display()))?;
    Ok(key)
}

fn load_config_key(key_path: &std::path::Path) -> anyhow::Result<[u8; CONFIG_KEY_BYTES]> {
    let encoded = fs::read_to_string(key_path)
        .with_context(|| format!("read config encryption key {}", key_path.display()))?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .context("decode config encryption key")?;
    decoded
        .try_into()
        .map_err(|_| anyhow!("config encryption key must contain 32 bytes"))
}

fn encrypt_config(path: &std::path::Path, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let key = load_or_create_config_key(path)?;
    let key = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, &key).map_err(|_| anyhow!("create config encryption key"))?,
    );
    let mut nonce = [0_u8; CONFIG_NONCE_BYTES];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce)
        .map_err(|_| anyhow!("generate config encryption nonce"))?;
    let mut ciphertext = plaintext.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::empty(),
        &mut ciphertext,
    )
    .map_err(|_| anyhow!("encrypt configuration"))?;
    serde_json::to_vec_pretty(&EncryptedConfig {
        encryption: CONFIG_ENCRYPTION.to_owned(),
        nonce: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce),
        ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
    })
    .context("serialize encrypted configuration")
}

fn decrypt_config_if_needed(path: &std::path::Path, raw: &[u8]) -> anyhow::Result<(Vec<u8>, bool)> {
    let Ok(envelope) = serde_json::from_slice::<EncryptedConfig>(raw) else {
        return Ok((raw.to_vec(), false));
    };
    anyhow::ensure!(
        envelope.encryption == CONFIG_ENCRYPTION,
        "unsupported config encryption {}",
        envelope.encryption
    );
    let nonce: [u8; CONFIG_NONCE_BYTES] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(envelope.nonce)
        .context("decode config encryption nonce")?
        .try_into()
        .map_err(|_| anyhow!("config encryption nonce must contain 12 bytes"))?;
    let mut ciphertext = base64::engine::general_purpose::STANDARD
        .decode(envelope.ciphertext)
        .context("decode encrypted configuration")?;
    let key_path = config_key_path(path)?;
    anyhow::ensure!(
        key_path.exists(),
        "config encryption key is missing: {}",
        key_path.display()
    );
    let key = load_config_key(&key_path)?;
    let key = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, &key).map_err(|_| anyhow!("create config decryption key"))?,
    );
    let plaintext = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut ciphertext,
        )
        .map_err(|_| {
            anyhow!("decrypt configuration; the config key is missing or does not match")
        })?;
    Ok((plaintext.to_vec(), true))
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
    let key_path = config_key_path(&path)?;
    if key_path.exists() {
        fs::remove_file(&key_path).with_context(|| format!("remove {}", key_path.display()))?;
    }
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
