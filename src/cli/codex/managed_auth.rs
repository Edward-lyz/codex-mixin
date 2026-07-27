use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const MANAGED_AUTH_KEY_PREFIX: &str = "codex-mixin-local-";
const MANAGED_BEDROCK_REGION: &str = "us-east-1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::cli) enum ManagedAuthMode {
    Official,
    CustomOnly,
}

#[derive(Debug)]
pub(in crate::cli) struct ManagedAuthTransaction {
    paths: ManagedAuthPaths,
    rollback: AuthRollback,
}

#[derive(Debug)]
enum AuthRollback {
    None,
    RestoreOriginal,
    RestoreManagedFake(Vec<u8>),
    RestoreUpgradedManagedFake(Vec<u8>),
    AdoptCurrentOnCommit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManagedFakeAuthKind {
    Bedrock,
    LegacyApiKey,
}

#[derive(Debug)]
struct ManagedAuthPaths {
    auth: PathBuf,
    backup: PathBuf,
    absent: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::cli) enum ManagedAuthUninstall {
    NotManaged,
    RemovedFake,
    RestoredBackup,
    PreservedChangedAuth { backup: Option<PathBuf> },
}

impl ManagedAuthTransaction {
    pub(in crate::cli) fn begin(config_path: &Path, mode: ManagedAuthMode) -> anyhow::Result<Self> {
        let paths = ManagedAuthPaths::from_config(config_path)?;
        match mode {
            ManagedAuthMode::Official => begin_official_auth(paths),
            ManagedAuthMode::CustomOnly => begin_custom_auth(paths),
        }
    }

    pub(in crate::cli) fn commit(self) -> anyhow::Result<()> {
        if matches!(
            self.rollback,
            AuthRollback::RestoreManagedFake(_) | AuthRollback::AdoptCurrentOnCommit
        ) {
            remove_restore_points(&self.paths)?;
        }
        Ok(())
    }

    pub(in crate::cli) fn rollback(self) -> anyhow::Result<()> {
        match self.rollback {
            AuthRollback::None | AuthRollback::AdoptCurrentOnCommit => Ok(()),
            AuthRollback::RestoreOriginal => restore_original_auth(&self.paths, true),
            AuthRollback::RestoreManagedFake(fake)
            | AuthRollback::RestoreUpgradedManagedFake(fake) => {
                write_private_atomic(&self.paths.auth, &fake)
            }
        }
    }
}

impl ManagedAuthPaths {
    fn from_config(config_path: &Path) -> anyhow::Result<Self> {
        let codex_home = config_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Codex config path has no parent"))?;
        let auth = codex_home.join("auth.json");
        Ok(Self {
            backup: sibling_path(&auth, "codex-mixin.backup"),
            absent: sibling_path(&auth, "codex-mixin.absent"),
            auth,
        })
    }

    fn has_restore_point(&self) -> bool {
        self.backup.exists() || self.absent.exists()
    }

    fn validate_restore_point(&self) -> anyhow::Result<()> {
        if self.backup.exists() && self.absent.exists() {
            anyhow::bail!(
                "conflicting Codex auth restore points: {} and {}",
                self.backup.display(),
                self.absent.display()
            );
        }
        Ok(())
    }
}

fn begin_custom_auth(paths: ManagedAuthPaths) -> anyhow::Result<ManagedAuthTransaction> {
    paths.validate_restore_point()?;
    if paths.has_restore_point() {
        match managed_fake_auth_kind(&paths.auth)? {
            Some(ManagedFakeAuthKind::Bedrock) => {
                return Ok(ManagedAuthTransaction {
                    paths,
                    rollback: AuthRollback::None,
                });
            }
            Some(ManagedFakeAuthKind::LegacyApiKey) => {
                let legacy_fake = fs::read(&paths.auth)?;
                write_managed_bedrock_auth(&paths.auth)?;
                return Ok(ManagedAuthTransaction {
                    paths,
                    rollback: AuthRollback::RestoreUpgradedManagedFake(legacy_fake),
                });
            }
            None => {}
        }
        anyhow::bail!(
            "Codex auth changed while a codex-mixin custom-mode restore point exists; \
             run `codex-mixin uninstall-codex` before reinstalling custom mode"
        );
    }

    create_auth_restore_point(&paths)?;
    if let Err(error) = write_managed_bedrock_auth(&paths.auth) {
        let cleanup = remove_restore_points(&paths);
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(anyhow::anyhow!(
                "{error}; failed to remove auth restore point: {cleanup_error}"
            )),
        };
    }
    Ok(ManagedAuthTransaction {
        paths,
        rollback: AuthRollback::RestoreOriginal,
    })
}

fn begin_official_auth(paths: ManagedAuthPaths) -> anyhow::Result<ManagedAuthTransaction> {
    paths.validate_restore_point()?;
    if !paths.has_restore_point() {
        return Ok(ManagedAuthTransaction {
            paths,
            rollback: AuthRollback::None,
        });
    }
    if !is_managed_fake_auth(&paths.auth)? {
        return Ok(ManagedAuthTransaction {
            paths,
            rollback: AuthRollback::AdoptCurrentOnCommit,
        });
    }

    let fake = fs::read(&paths.auth)?;
    restore_original_auth(&paths, false)?;
    Ok(ManagedAuthTransaction {
        paths,
        rollback: AuthRollback::RestoreManagedFake(fake),
    })
}

pub(in crate::cli) fn uninstall_managed_custom_auth(
    config_path: &Path,
) -> anyhow::Result<ManagedAuthUninstall> {
    let paths = ManagedAuthPaths::from_config(config_path)?;
    paths.validate_restore_point()?;
    if !paths.has_restore_point() {
        return Ok(ManagedAuthUninstall::NotManaged);
    }

    if paths.auth.exists() && !is_managed_fake_auth(&paths.auth)? {
        return Ok(ManagedAuthUninstall::PreservedChangedAuth {
            backup: paths.backup.exists().then(|| paths.backup.clone()),
        });
    }

    let restored_backup = paths.backup.exists();
    restore_original_auth(&paths, true)?;
    Ok(if restored_backup {
        ManagedAuthUninstall::RestoredBackup
    } else {
        ManagedAuthUninstall::RemovedFake
    })
}

pub(in crate::cli) fn is_managed_fake_auth(path: &Path) -> anyhow::Result<bool> {
    Ok(managed_fake_auth_kind(path)?.is_some())
}

fn managed_fake_auth_kind(path: &Path) -> anyhow::Result<Option<ManagedFakeAuthKind>> {
    if !path.exists() {
        return Ok(None);
    }
    let auth: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    let mode = auth.get("auth_mode").and_then(serde_json::Value::as_str);
    if mode == Some("bedrockApiKey")
        && auth
            .pointer("/bedrock_api_key/api_key")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|key| key.starts_with(MANAGED_AUTH_KEY_PREFIX))
    {
        return Ok(Some(ManagedFakeAuthKind::Bedrock));
    }
    if mode == Some("apikey")
        && auth
            .get("OPENAI_API_KEY")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|key| key.starts_with(MANAGED_AUTH_KEY_PREFIX))
    {
        return Ok(Some(ManagedFakeAuthKind::LegacyApiKey));
    }
    Ok(None)
}

fn write_managed_bedrock_auth(path: &Path) -> anyhow::Result<()> {
    let fake_auth = serde_json::to_vec_pretty(&serde_json::json!({
        "auth_mode": "bedrockApiKey",
        "bedrock_api_key": {
            "api_key": format!("{MANAGED_AUTH_KEY_PREFIX}{}", uuid::Uuid::new_v4()),
            "region": MANAGED_BEDROCK_REGION,
        },
    }))?;
    write_private_atomic(path, &fake_auth)
}

fn create_auth_restore_point(paths: &ManagedAuthPaths) -> anyhow::Result<()> {
    if let Some(parent) = paths.auth.parent() {
        fs::create_dir_all(parent)?;
    }
    if paths.auth.exists() {
        let contents = fs::read(&paths.auth)?;
        write_private_atomic(&paths.backup, &contents)
    } else {
        write_private_atomic(&paths.absent, b"")
    }
}

fn restore_original_auth(
    paths: &ManagedAuthPaths,
    remove_restore_point: bool,
) -> anyhow::Result<()> {
    if paths.backup.exists() {
        let backup = fs::read(&paths.backup)?;
        write_private_atomic(&paths.auth, &backup)?;
    } else if paths.absent.exists() && paths.auth.exists() {
        fs::remove_file(&paths.auth)?;
    }
    if remove_restore_point {
        remove_restore_points(paths)?;
    }
    Ok(())
}

fn remove_restore_points(paths: &ManagedAuthPaths) -> anyhow::Result<()> {
    for path in [&paths.backup, &paths.absent] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn write_private_atomic(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("auth.json");
    let temporary = path.with_file_name(format!(
        "{file_name}.tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("auth.json");
    path.with_file_name(format!("{file_name}.{suffix}"))
}
