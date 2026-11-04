//! Durable recovery for the existing Codex install/uninstall commands.
//! The journal is written before any Codex file and removed only after cleanup.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};

use super::managed_auth::{managed_apikey_auth, managed_auth_bytes};
use super::managed_config::{
    CodexInstallPaths, absolute_path, managed_catalog_path, sibling_path_with_extra_extension,
};

const FORMAT_VERSION: u32 = 1;
const MANAGED_SKILL_PREFIX: &str = "codex-mixin managed imagegen skill";
const OFFICIAL_CONFIG: &str = "model_provider = \"openai\"\n";

#[derive(Deserialize, Serialize)]
pub(super) struct ManagedCodexRestore {
    version: u32,
    config: PathBuf,
    catalog: PathBuf,
    config_before: Option<Vec<u8>>,
    catalog_before: Option<Vec<u8>>,
    auth_before: Option<Vec<u8>>,
    managed_config: Vec<u8>,
    managed_catalog: Option<Vec<u8>>,
    pub(super) codex_oauth_proxy: bool,
}

#[derive(Deserialize, Serialize)]
struct FileChange {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
    cleanup: bool,
}

#[derive(Deserialize, Serialize)]
struct CodexJournal {
    version: u32,
    config: PathBuf,
    changes: Vec<FileChange>,
}

pub(super) fn directory(config: &Path) -> PathBuf {
    let key = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        config.as_os_str().as_encoded_bytes(),
    );
    super::super::runtime::state_dir()
        .join("codex-restore")
        .join(key.to_string())
}

pub(in crate::cli) fn integration_status(
    config: &Path,
) -> anyhow::Result<codex_mixin::clients::IntegrationStatus> {
    let mut status = codex_mixin::application::agents::integration_status("codex", config, None)?;
    match ManagedCodexRestore::load(config) {
        Ok(Some(record)) if status.state != "managed" => {
            status.state = "drift";
            status.mode = Some(if record.codex_oauth_proxy {
                "codex_oauth_proxy"
            } else {
                "custom_only"
            });
            status.detail = "Codex files changed or are missing; Mixin recovery data is available.";
        }
        Err(error) => {
            status.state = "error";
            status.detail = "Cannot read the Codex recovery record.";
            status.error = Some(format!("{error:#}"));
        }
        _ => {}
    }
    Ok(status)
}

pub(super) fn lock_path(config: &Path) -> PathBuf {
    // All config files in one Codex home share auth.json, hence one lock.
    directory(&config.with_file_name("auth.json")).join("integration.lock")
}

fn owner_path(config: &Path) -> PathBuf {
    lock_path(config).with_file_name("owner.json")
}

fn record_path(config: &Path) -> PathBuf {
    directory(config).join("restore.json")
}

fn journal_path(config: &Path) -> PathBuf {
    lock_path(config).with_file_name("transaction.json")
}

fn read_file(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            !metadata.file_type().is_symlink(),
            "refusing to replace a symlink at {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    }
    fs::read(path)
        .map(Some)
        .with_context(|| format!("read {}", path.display()))
}

fn replace(path: &Path, contents: Option<&[u8]>) -> anyhow::Result<()> {
    match contents {
        Some(contents) => codex_mixin::platform::write_owner_only(path, contents),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
        },
    }
}

impl FileChange {
    fn new(path: PathBuf, after: Option<Vec<u8>>, cleanup: bool) -> anyhow::Result<Self> {
        Ok(Self {
            before: read_file(&path)?,
            path,
            after,
            cleanup,
        })
    }
}

impl ManagedCodexRestore {
    pub(super) fn load(config: &Path) -> anyhow::Result<Option<Self>> {
        let Some(bytes) = read_file(&record_path(config))? else {
            return Ok(None);
        };
        let record: Self = serde_json::from_slice(&bytes).context("decode Codex restore record")?;
        ensure!(
            record.version == FORMAT_VERSION && record.config == config,
            "Codex restore record has an unsupported version or a different config path"
        );
        validate_paths(&record.config, &record.catalog)?;
        Ok(Some(record))
    }

    pub(super) fn install_config(&self, current: &str) -> anyhow::Result<String> {
        self.check_config(current.as_bytes())?;
        if current.is_empty() {
            return String::from_utf8(self.config_before.clone().unwrap_or_default())
                .context("decode original Codex config");
        }
        Ok(current.to_owned())
    }

    fn check_config(&self, current: &[u8]) -> anyhow::Result<()> {
        ensure!(
            current.is_empty()
                || self.config_before.as_deref() == Some(current)
                || std::str::from_utf8(current)
                    .is_ok_and(codex_mixin::clients::codex::document_is_managed),
            "Codex config changed outside Mixin; recovery data is retained at {}",
            directory(&self.config).display()
        );
        Ok(())
    }

    fn prepare(paths: &CodexInstallPaths, raw: &str) -> anyhow::Result<Self> {
        validate_paths(&paths.config, &paths.catalog)?;
        if let Some(record) = Self::load(&paths.config)? {
            ensure!(
                record.catalog == paths.catalog,
                "disconnect Codex before changing its managed catalog path"
            );
            record.check_config(raw.as_bytes())?;
            return Ok(record);
        }
        let managed = codex_mixin::clients::codex::document_is_managed(raw);
        let legacy_config = legacy_original(&paths.config)?;
        let legacy = managed || (raw.is_empty() && legacy_config.is_some());
        let config_before = if legacy {
            legacy_config.with_context(|| {
                format!(
                    "missing pre-install Codex backup for {}; cannot reconstruct the original config",
                    paths.config.display()
                )
            })?
        } else {
            read_file(&paths.config)?
        };
        let auth_path = paths.config.with_file_name("auth.json");
        let current_auth = read_file(&auth_path)?;
        let fake = managed_auth_bytes(current_auth.as_deref())?;
        let auth_before = if legacy && fake {
            legacy_original(&auth_path)?
                .context("managed Codex auth is missing its pre-install backup")?
        } else if legacy && current_auth.is_none() {
            legacy_original(&auth_path)?.flatten()
        } else {
            current_auth
        };
        ensure!(
            !fake || legacy,
            "managed Codex auth has no matching config recovery record"
        );
        let codex_oauth_proxy = if managed {
            let document = raw.parse::<toml_edit::DocumentMut>()?;
            ensure!(
                managed_catalog_path(&document, &paths.config)? == paths.catalog,
                "managed Codex catalog differs from the requested catalog path"
            );
            codex_mixin::clients::codex::managed_provider_id(&document)?
                == codex_mixin::CODEX_MIXIN_PROVIDER
        } else {
            !fake
        };
        let current_catalog = read_file(&paths.catalog)?;
        if legacy && let Some(catalog) = &current_catalog {
            let catalog = serde_json::from_slice(catalog)?;
            codex_mixin::catalog::official::validate_catalog(&catalog)?;
        }
        Ok(Self {
            version: FORMAT_VERSION,
            config: paths.config.clone(),
            catalog: paths.catalog.clone(),
            config_before,
            catalog_before: (!legacy).then(|| current_catalog.clone()).flatten(),
            auth_before,
            managed_config: raw.as_bytes().to_vec(),
            managed_catalog: legacy.then_some(current_catalog).flatten(),
            codex_oauth_proxy,
        })
    }
}

fn validate_paths(config: &Path, catalog: &Path) -> anyhow::Result<()> {
    let auth = config.with_file_name("auth.json");
    let state = absolute_path(super::super::runtime::state_dir())?;
    let gateway_config = absolute_path(codex_mixin::config::stored_config_path())?;
    let mut gateway_key = gateway_config.as_os_str().to_os_string();
    gateway_key.push(".key");
    let reserved = [
        gateway_config,
        PathBuf::from(gateway_key),
        absolute_path(super::switch::switch_state_path())?,
        absolute_path(super::super::runtime::runtime_metadata_path())?,
        absolute_path(super::super::runtime::daemon_metadata_path())?,
    ];
    ensure!(
        config.is_absolute()
            && catalog.is_absolute()
            && config != catalog
            && config != auth
            && catalog != auth
            && reserved
                .iter()
                .all(|path| config != path && catalog != path)
            && !config.starts_with(state.join("codex-restore"))
            && !catalog.starts_with(state.join("codex-restore")),
        "Codex config, auth, catalog and Mixin recovery paths must be distinct"
    );
    Ok(())
}

fn legacy_original(path: &Path) -> anyhow::Result<Option<Option<Vec<u8>>>> {
    let backup = read_file(&sibling_path_with_extra_extension(
        path,
        "codex-mixin.backup",
    ))?;
    let absent = read_file(&sibling_path_with_extra_extension(
        path,
        "codex-mixin.absent",
    ))?;
    ensure!(
        backup.is_none() || absent.is_none(),
        "conflicting legacy Codex restore points for {}",
        path.display()
    );
    Ok(backup.map(Some).or_else(|| absent.map(|_| None)))
}

fn legacy_cleanup(config: &Path, changes: &mut Vec<FileChange>) -> anyhow::Result<()> {
    for path in [config.to_path_buf(), config.with_file_name("auth.json")] {
        for suffix in ["codex-mixin.backup", "codex-mixin.absent"] {
            let legacy = sibling_path_with_extra_extension(&path, suffix);
            if let Some(contents) = read_file(&legacy)? {
                let name = legacy
                    .file_name()
                    .context("legacy backup has no file name")?;
                // Retain old/stale originals independently of the Codex home.
                changes.push(FileChange::new(
                    directory(config).join("legacy").join(name),
                    Some(contents),
                    false,
                )?);
                changes.push(FileChange::new(legacy, None, true)?);
            }
        }
    }
    Ok(())
}

pub(super) fn install(
    paths: &CodexInstallPaths,
    raw: &str,
    catalog: &[u8],
    config: &[u8],
    codex_oauth_proxy: bool,
    clear_switch: bool,
    validate: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut record = ManagedCodexRestore::prepare(paths, raw)?;
    if ManagedCodexRestore::load(&paths.config)?.is_some() {
        let current_catalog = read_file(&paths.catalog)?;
        ensure!(
            current_catalog.is_none()
                || current_catalog == record.managed_catalog
                || current_catalog == record.catalog_before,
            "Codex model catalog changed outside Mixin; disconnect before reconnecting"
        );
    }
    let owner = owner_path(&paths.config);
    if let Some(bytes) = read_file(&owner)? {
        let owned_config: PathBuf = serde_json::from_slice(&bytes)?;
        ensure!(
            owned_config == paths.config,
            "another Codex config in this home is managed: {}; disconnect it first",
            owned_config.display()
        );
    }
    let auth_path = paths.config.with_file_name("auth.json");
    let current_auth = read_file(&auth_path)?;
    let fake = managed_auth_bytes(current_auth.as_deref())?;
    let desired_auth = if codex_oauth_proxy {
        if fake || current_auth.is_none() {
            record.auth_before.clone()
        } else {
            current_auth.clone()
        }
    } else {
        ensure!(
            record.codex_oauth_proxy || fake || current_auth.is_none(),
            "Codex auth changed during custom mode; disconnect before reconnecting"
        );
        Some(managed_apikey_auth()?)
    };
    if !fake && current_auth.is_some() {
        record.auth_before = current_auth.clone();
    }
    record.codex_oauth_proxy = codex_oauth_proxy;
    record.managed_config = config.to_vec();
    record.managed_catalog = Some(catalog.to_vec());
    let mut changes = vec![
        FileChange::new(
            super::managed_config::managed_backup_path(&paths.config),
            record.config_before.clone(),
            false,
        )?,
        FileChange::new(
            super::managed_config::managed_absent_marker_path(&paths.config),
            record.config_before.is_none().then(Vec::new),
            false,
        )?,
        FileChange::new(
            record_path(&paths.config),
            Some(serde_json::to_vec(&record)?),
            false,
        )?,
        FileChange::new(owner, Some(serde_json::to_vec(&paths.config)?), false)?,
        FileChange::new(auth_path, desired_auth, false)?,
        FileChange::new(paths.catalog.clone(), Some(catalog.to_vec()), false)?,
        FileChange::new(paths.config.clone(), Some(config.to_vec()), false)?,
    ];
    legacy_cleanup(&paths.config, &mut changes)?;
    if clear_switch {
        changes.push(FileChange::new(
            super::switch::switch_state_path(),
            None,
            true,
        )?);
    }
    apply(&paths.config, changes, validate)
}

/// Disconnect resets Codex to the official provider instead of restoring a
/// saved config: old backups may be missing or still point at Mixin. Returns
/// cleanup warnings and the best-effort history normalization result.
pub(super) fn uninstall(
    config: &Path,
    explicit_catalog: Option<PathBuf>,
    clear_switch: bool,
) -> anyhow::Result<(Vec<String>, super::history::HistoryMigrationOutcome)> {
    let explicit_catalog = explicit_catalog.map(absolute_path).transpose()?;
    if let Some(bytes) = read_file(&owner_path(config))? {
        let owned_config: PathBuf = serde_json::from_slice(&bytes)?;
        ensure!(
            owned_config == config,
            "another Codex config in this home is managed: {}",
            owned_config.display()
        );
    }
    let codex_home = config.parent().context("Codex config has no parent")?;
    let mut warnings = Vec::new();
    let record = ManagedCodexRestore::load(config).unwrap_or_else(|error| {
        warnings.push(format!(
            "ignored unreadable Codex recovery record: {error:#}"
        ));
        None
    });
    let current_config = read_file(config)?;
    let raw = String::from_utf8_lossy(current_config.as_deref().unwrap_or_default());
    let default_catalog = codex_home.join("model-catalogs/mixin-models.json");
    let mut catalog = record.as_ref().map(|record| record.catalog.clone());
    if catalog.is_none() && codex_mixin::clients::codex::document_is_managed(&raw) {
        match raw
            .parse()
            .map_err(anyhow::Error::from)
            .and_then(|document| managed_catalog_path(&document, config))
        {
            Ok(path) => catalog = Some(path),
            Err(error) => warnings.push(format!("cannot locate the managed catalog: {error:#}")),
        }
    }
    let catalog = catalog.unwrap_or_else(|| default_catalog.clone());
    if let Some(explicit) = &explicit_catalog {
        ensure!(
            *explicit == catalog,
            "explicit catalog does not match the managed catalog {}",
            catalog.display()
        );
    }
    let mut changes = vec![FileChange::new(
        config.to_path_buf(),
        Some(OFFICIAL_CONFIG.as_bytes().to_vec()),
        false,
    )?];
    if let Some(current) = read_file(&catalog)? {
        let owned = catalog == default_catalog
            || explicit_catalog.is_some()
            || record
                .as_ref()
                .is_some_and(|record| record.managed_catalog.as_ref() == Some(&current));
        if owned {
            changes.push(FileChange::new(catalog, None, true)?);
        } else {
            warnings.push(format!(
                "left unrecognized model catalog in place: {}",
                catalog.display()
            ));
        }
    }
    let auth_path = config.with_file_name("auth.json");
    let current_auth = read_file(&auth_path)?;
    match managed_auth_bytes(current_auth.as_deref()) {
        Ok(true) => changes.push(FileChange::new(auth_path, None, true)?),
        Ok(false) => {}
        Err(error) => warnings.push(format!(
            "left unreadable {} in place: {error:#}",
            auth_path.display()
        )),
    }
    // Include skill files in this journal rather than restoring them before
    // config/auth preflight, which used to leave a partially disconnected agent.
    let skill = codex_home.join("skills/.system/imagegen");
    for relative in ["SKILL.md", "scripts/image_gen.py"] {
        let path = skill.join(relative);
        let mut backup_name = path.as_os_str().to_owned();
        backup_name.push(".codex-mixin.bak");
        let backup = PathBuf::from(backup_name);
        if let Some(original) = read_file(&backup)? {
            let current = read_file(&path)?;
            if current
                .as_deref()
                .is_none_or(|bytes| String::from_utf8_lossy(bytes).contains(MANAGED_SKILL_PREFIX))
            {
                changes.push(FileChange::new(path, Some(original), false)?);
            }
            changes.push(FileChange::new(backup, None, true)?);
        }
    }
    // Old restore points stay as rescue copies only; they no longer drive
    // disconnect or make the official config look drifted.
    legacy_cleanup(config, &mut changes)?;
    for path in [owner_path(config), record_path(config)] {
        changes.push(FileChange::new(path, None, true)?);
    }
    if clear_switch {
        changes.push(FileChange::new(
            super::switch::switch_state_path(),
            None,
            true,
        )?);
    }
    apply(config, changes, || {
        ensure!(
            !codex_mixin::clients::codex::is_managed(config)?,
            "reset Codex config is still managed by Mixin"
        );
        Ok(())
    })?;
    let history = super::history::migrate_history_to_provider(codex_home, "openai")?;
    Ok((warnings, history))
}

fn apply(
    config: &Path,
    changes: Vec<FileChange>,
    validate: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut paths = std::collections::HashSet::new();
    ensure!(
        changes.iter().all(|change| paths.insert(&change.path)),
        "Codex transaction contains conflicting file paths"
    );
    let journal = CodexJournal {
        version: FORMAT_VERSION,
        config: config.to_path_buf(),
        changes: changes
            .into_iter()
            .filter(|change| change.before != change.after)
            .collect(),
    };
    let path = journal_path(config);
    codex_mixin::platform::ensure_owner_only_dir(&directory(config))?;
    codex_mixin::platform::write_owner_only(&path, &serde_json::to_vec(&journal)?)?;
    let mut validate = Some(validate);
    let result = (|| {
        for cleanup in [false, true] {
            if cleanup && let Some(validate) = validate.take() {
                validate()?;
            }
            for change in journal
                .changes
                .iter()
                .filter(|change| change.cleanup == cleanup)
            {
                ensure!(
                    read_file(&change.path)? == change.before,
                    "Codex file changed during transaction: {}",
                    change.path.display()
                );
                replace(&change.path, change.after.as_deref())?;
                ensure!(
                    read_file(&change.path)? == change.after,
                    "Codex transaction validation failed: {}",
                    change.path.display()
                );
            }
        }
        replace(&path, None)
    })();
    if let Err(error) = result {
        return match rollback(&journal) {
            Ok(()) => {
                replace(&path, None)?;
                Err(error).context("Codex transaction failed; all changes rolled back")
            }
            Err(rollback_error) => Err(anyhow::anyhow!(
                "{error:#}; rollback incomplete: {rollback_error:#}; recovery journal retained at {}",
                path.display()
            )),
        };
    }
    Ok(())
}

fn rollback(journal: &CodexJournal) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    for change in journal.changes.iter().rev() {
        let result = (|| {
            let current = read_file(&change.path)?;
            if current == change.before {
                return Ok(());
            }
            ensure!(
                current == change.after,
                "external edit prevents rollback of {}",
                change.path.display()
            );
            replace(&change.path, change.before.as_deref())
        })();
        if let Err(error) = result {
            errors.push(format!("{error:#}"));
        }
    }
    ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}

pub(super) fn recover(config: &Path) -> anyhow::Result<()> {
    let path = journal_path(config);
    let Some(bytes) = read_file(&path)? else {
        return Ok(());
    };
    let journal: CodexJournal = serde_json::from_slice(&bytes).context("decode Codex journal")?;
    ensure!(
        journal.version == FORMAT_VERSION && journal.config.parent() == config.parent(),
        "Codex recovery journal has an unsupported version or a different config path"
    );
    rollback(&journal).with_context(|| format!("recover transaction at {}", path.display()))?;
    replace(&path, None)
}

pub(super) fn refresh(
    config: &Path,
    catalog: &Path,
    config_contents: Option<Vec<u8>>,
    catalog_contents: Vec<u8>,
) -> anyhow::Result<bool> {
    validate_paths(config, catalog)?;
    let mut changes = vec![FileChange::new(
        catalog.to_path_buf(),
        Some(catalog_contents.clone()),
        false,
    )?];
    if let Some(contents) = &config_contents {
        changes.push(FileChange::new(
            config.to_path_buf(),
            Some(contents.clone()),
            false,
        )?);
    }
    let changed = changes.iter().any(|change| change.before != change.after);
    let raw_config = String::from_utf8(read_file(config)?.unwrap_or_default())
        .context("decode managed Codex config")?;
    let mut record = if let Some(record) = ManagedCodexRestore::load(config)? {
        ensure!(
            record.catalog == catalog,
            "Codex catalog path changed outside Mixin"
        );
        let current = read_file(catalog)?;
        ensure!(
            current.is_none()
                || current == record.managed_catalog
                || current == record.catalog_before,
            "Codex model catalog changed outside Mixin; recovery data is retained"
        );
        record
    } else if legacy_original(config)?.is_some() {
        // Older 0.7.6 installs only left sibling restore points. Adopt those
        // points before refreshing anything so a later uninstall cannot lose
        // the original config or auth after this upgrade.
        let paths = CodexInstallPaths {
            config: config.to_path_buf(),
            catalog: catalog.to_path_buf(),
            models_cache: config.with_file_name("models_cache.json"),
        };
        ManagedCodexRestore::prepare(&paths, &raw_config)?
    } else {
        // A managed file without a legacy restore point cannot be safely
        // assigned an original value. Keep the historical refresh behavior;
        // uninstall will report that the original is unrecoverable.
        apply(config, changes, || Ok(()))?;
        return Ok(changed);
    };
    record.managed_catalog = Some(catalog_contents);
    if let Some(contents) = config_contents {
        record.managed_config = contents;
    }
    changes.extend([
        FileChange::new(
            super::managed_config::managed_backup_path(config),
            record.config_before.clone(),
            false,
        )?,
        FileChange::new(
            super::managed_config::managed_absent_marker_path(config),
            record.config_before.is_none().then(Vec::new),
            false,
        )?,
        FileChange::new(
            record_path(config),
            Some(serde_json::to_vec(&record)?),
            false,
        )?,
        FileChange::new(owner_path(config), Some(serde_json::to_vec(config)?), false)?,
    ]);
    legacy_cleanup(config, &mut changes)?;
    apply(config, changes, || Ok(()))?;
    Ok(changed)
}
