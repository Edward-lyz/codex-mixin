use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_yaml::{Mapping, Value};

use crate::platform::{ensure_owner_only_dir, restrict_owner_only_file, write_atomic_if_changed};

const PROVIDER_ID: &str = "codex-mixin";
const API_KEY_ENV: &str = "CODEX_MIXIN_GATEWAY_API_KEY";
const API_PROTOCOL: &str = "openai-responses";
const CREDENTIALS_VERSION_FIELD: &str = "version";
const CREDENTIALS_REFS_FIELD: &str = "refs";
const CREDENTIALS_RECORDS_FIELD: &str = "records";
const LLM_ENTRY_ID: &str = "llm-pi-ai";
const PROFILE_PATCH_FILE: &str = "cordis.patch.yml";
const GLOBAL_TAG_PREFIX: &str = "codex-mixin-global/";

#[cfg(test)]
mod tests;

/// DSH `.credentials.yaml` layout version. DSH 0.1.5+ requires a versioned
/// document whose only top-level keys are `version`, `refs`, and `records`;
/// every credential reference nests under `refs`. Writing a bare top-level key
/// makes DSH reject the whole file at load ("unknown top-level key").
const CREDENTIALS_DOCUMENT_VERSION: i64 = 1;

fn is_credential_ref_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

/// Upgrade a pre-release flat document by moving every credential reference
/// under `refs`. Moving only our own key would stamp a mixed document as
/// version 1 while leaving other flat keys at the root, which DSH rejects both
/// at boot and during hot reload.
fn migrate_flat_credentials(root: &mut Mapping) -> anyhow::Result<()> {
    let version_key = Value::String(CREDENTIALS_VERSION_FIELD.to_owned());
    if root.is_empty() || root.contains_key(&version_key) {
        return Ok(());
    }

    for (key, value) in root.iter() {
        let key = key
            .as_str()
            .context("flat DSH credential names must be strings")?;
        anyhow::ensure!(
            is_credential_ref_name(key),
            "flat DSH credential name {key:?} is not a POSIX identifier"
        );
        let value = value
            .as_str()
            .with_context(|| format!("flat DSH credential {key:?} must be a string"))?;
        anyhow::ensure!(!value.is_empty(), "flat DSH credential {key:?} is empty");
    }

    let refs = std::mem::take(root);
    root.insert(
        version_key,
        Value::Number(CREDENTIALS_DOCUMENT_VERSION.into()),
    );
    root.insert(
        Value::String(CREDENTIALS_REFS_FIELD.to_owned()),
        Value::Mapping(refs),
    );
    Ok(())
}

/// Insert or update one credential reference under the versioned `refs` section,
/// creating the `version`/`refs` scaffolding when absent.
///
/// Every DSH release reads this layout: the versioned reader is present as far
/// back as `dsh-v0.1.2-alpha.3`, and the flat top-level layout DSH calls
/// "pre-release" is only upgraded at boot, never on the watcher's hot reload.
/// Writing flat would therefore be silently dropped by a running DSH, so the
/// versioned layout is the only form that works both at boot and live.
fn set_credential_ref(credentials: &mut Value, key: &str, value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!value.is_empty(), "DSH credential {key:?} is empty");
    let root = credentials
        .as_mapping_mut()
        .context("DSH credentials must be a YAML mapping")?;
    migrate_flat_credentials(root)?;
    // A pre-0.1.5 codex-mixin wrote the key at the document root, which DSH
    // rejects as an unknown top-level key. Drop it as part of moving the value
    // under `refs`, or the rewritten file stays unloadable.
    root.remove(Value::String(key.to_owned()));
    let version = root
        .entry(Value::String(CREDENTIALS_VERSION_FIELD.to_owned()))
        .or_insert_with(|| Value::Number(CREDENTIALS_DOCUMENT_VERSION.into()));
    anyhow::ensure!(
        version.as_i64() == Some(CREDENTIALS_DOCUMENT_VERSION),
        "unsupported DSH credentials version"
    );
    for field in root.keys() {
        let field = field
            .as_str()
            .context("DSH credentials field names must be strings")?;
        anyhow::ensure!(
            matches!(
                field,
                CREDENTIALS_VERSION_FIELD | CREDENTIALS_REFS_FIELD | CREDENTIALS_RECORDS_FIELD
            ),
            "unknown DSH credentials field {field:?}"
        );
    }
    let refs_slot = root
        .entry(Value::String(CREDENTIALS_REFS_FIELD.to_owned()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    // A file whose last reference was hand-removed keeps a childless `refs:`,
    // which parses as null rather than an empty mapping. Treat it as empty so
    // the write proceeds instead of bailing on a well-formed document.
    if refs_slot.is_null() {
        *refs_slot = Value::Mapping(Mapping::new());
    }
    let refs = refs_slot
        .as_mapping_mut()
        .context("DSH credentials refs must be a YAML mapping")?;
    refs.insert(
        Value::String(key.to_owned()),
        Value::String(value.to_owned()),
    );
    Ok(())
}

/// Remove one credential reference, clearing both the versioned `refs` entry and
/// any pre-0.1.5 top-level entry left by an older codex-mixin.
fn remove_credential_ref(credentials: &mut Value, key: &str) -> anyhow::Result<()> {
    let root = credentials
        .as_mapping_mut()
        .context("DSH credentials must be a YAML mapping")?;
    root.remove(Value::String(key.to_owned()));
    if let Some(refs) = root
        .get_mut(Value::String("refs".to_owned()))
        .and_then(Value::as_mapping_mut)
    {
        refs.remove(Value::String(key.to_owned()));
    }
    Ok(())
}

pub fn install(
    dsh_home: &Path,
    bind: SocketAddr,
    models: Vec<Value>,
    client_key: &str,
) -> anyhow::Result<bool> {
    ensure_owner_only_dir(dsh_home)?;
    // Keep the legacy file for 0.1.x, and write the watched profile layer for
    // 0.2.x. Validate every document before changing credentials or settings.
    let profile = provider_profile(bind, models);
    let mut documents = read_settings(dsh_home, false)?;
    for document in &mut documents {
        for providers in document.providers(true)? {
            providers.insert(Value::String(PROVIDER_ID.to_owned()), profile.clone());
        }
    }

    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
    set_credential_ref(&mut credentials, API_KEY_ENV, client_key)?;
    let credentials_changed = write_yaml(&credentials_path, &credentials)?;
    let mut changed = credentials_changed;
    for document in documents {
        ensure_owner_only_dir(
            document
                .path
                .parent()
                .context("DSH settings parent missing")?,
        )?;
        changed |= document.write()?;
    }
    Ok(changed)
}

pub fn uninstall(dsh_home: &Path) -> anyhow::Result<()> {
    let mut documents = read_settings(dsh_home, true)?;
    let mut removed = false;
    let mut remove_key = false;
    let mut updates = Vec::new();
    for document in &mut documents {
        let mut changed = false;
        for providers in document.providers(false)? {
            if let Some(profile) = providers.remove(Value::String(PROVIDER_ID.to_owned())) {
                changed = true;
                remove_key |= profile.get("apiKeyEnv").and_then(Value::as_str) == Some(API_KEY_ENV);
            }
        }
        removed |= changed;
        if changed {
            updates.push(document);
        }
    }
    anyhow::ensure!(removed, "DSH provider codex-mixin is not installed");
    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
    if remove_key {
        remove_credential_ref(&mut credentials, API_KEY_ENV)?;
    }
    // Remove live references before revoking the shared credential.
    for document in updates {
        document.write()?;
    }
    if remove_key && credentials_path.exists() {
        write_yaml(&credentials_path, &credentials)?;
    }
    Ok(())
}

pub fn is_managed(dsh_home: &Path) -> anyhow::Result<bool> {
    let mut managed = false;
    // `.imported` is a migration backup, never evidence of an active provider.
    for mut document in read_settings(dsh_home, false)? {
        managed |= document.providers(false)?.iter().any(|providers| {
            providers
                .get(Value::String(PROVIDER_ID.to_owned()))
                .and_then(|provider| provider.get("displayName"))
                .and_then(Value::as_str)
                == Some("Codex Mixin")
        });
    }
    Ok(managed)
}

struct SettingsDocument {
    path: PathBuf,
    contents: Value,
    profile: bool,
}

impl SettingsDocument {
    fn write(&self) -> anyhow::Result<bool> {
        if !self.profile {
            return write_yaml(&self.path, &self.contents);
        }
        let mut contents = self.contents.clone();
        let marker = format!("codex-mixin-tag-{}-", uuid::Uuid::new_v4());
        preserve_global_tags(&mut contents, &marker);
        let yaml = serde_yaml::to_string(&contents)?.replace(&format!("!{marker}"), "!!");
        let changed = write_atomic_if_changed(&self.path, yaml.as_bytes())?;
        restrict_owner_only_file(&self.path)?;
        Ok(changed)
    }

    fn providers(&mut self, create: bool) -> anyhow::Result<Vec<&mut Mapping>> {
        let configs = if self.profile {
            let entries = self.contents.as_sequence_mut().with_context(|| {
                format!(
                    "DSH profile patch must be a YAML sequence: {}",
                    self.path.display()
                )
            })?;
            if create
                && !entries
                    .iter()
                    .any(|entry| entry.get("id").and_then(Value::as_str) == Some(LLM_ENTRY_ID))
            {
                entries.push(Value::Mapping(Mapping::from_iter([(
                    Value::String("id".to_owned()),
                    Value::String(LLM_ENTRY_ID.to_owned()),
                )])));
            }
            let mut configs = Vec::new();
            for entry in entries
                .iter_mut()
                .filter(|entry| entry.get("id").and_then(Value::as_str) == Some(LLM_ENTRY_ID))
            {
                let entry = entry
                    .as_mapping_mut()
                    .context("DSH profile entry must be a mapping")?;
                if create || entry.contains_key(Value::String("config".to_owned())) {
                    configs.push(
                        entry
                            .entry(Value::String("config".to_owned()))
                            .or_insert_with(|| Value::Mapping(Mapping::new())),
                    );
                }
            }
            configs
        } else {
            let root = self
                .contents
                .as_mapping_mut()
                .context("DSH settings must be a YAML mapping")?;
            if !create && !root.contains_key(Value::String(LLM_ENTRY_ID.to_owned())) {
                return Ok(Vec::new());
            }
            vec![
                root.entry(Value::String(LLM_ENTRY_ID.to_owned()))
                    .or_insert_with(|| Value::Mapping(Mapping::new())),
            ]
        };
        let mut mappings = Vec::new();
        for config in configs {
            let config = config
                .as_mapping_mut()
                .context("DSH llm-pi-ai config must be a YAML mapping")?;
            if !create && !config.contains_key(Value::String("providers".to_owned())) {
                continue;
            }
            let providers = config
                .entry(Value::String("providers".to_owned()))
                .or_insert_with(|| Value::Mapping(Mapping::new()));
            mappings.push(
                providers
                    .as_mapping_mut()
                    .context("DSH llm-pi-ai providers must be a YAML mapping")?,
            );
        }
        Ok(mappings)
    }
}

fn read_settings(dsh_home: &Path, include_imported: bool) -> anyhow::Result<Vec<SettingsDocument>> {
    let mut paths = vec![(dsh_home.join("settings.yaml"), false)];
    if include_imported {
        paths.push((dsh_home.join("settings.yaml.imported"), false));
    }
    let profiles = dsh_home.join("profiles");
    let mut profile_dirs = vec![profiles.join("desktop")];
    if profiles.exists() {
        for entry in std::fs::read_dir(&profiles)
            .with_context(|| format!("read DSH profiles {}", profiles.display()))?
        {
            let entry = entry?;
            let name = entry.file_name();
            if entry.file_type()?.is_dir()
                && name != "node_modules"
                && !name.to_string_lossy().starts_with('.')
            {
                profile_dirs.push(entry.path());
            }
        }
    }
    profile_dirs.sort();
    profile_dirs.dedup();
    paths.extend(
        profile_dirs
            .into_iter()
            .map(|dir| (dir.join(PROFILE_PATCH_FILE), true)),
    );
    paths
        .into_iter()
        .map(|(path, profile)| {
            let contents = if profile {
                read_profile(&path)?
            } else {
                read_yaml(&path, "DSH settings")?
            };
            Ok(SettingsDocument {
                path,
                contents,
                profile,
            })
        })
        .collect()
}

fn read_profile(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(Value::Sequence(Vec::new()));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read DSH profile {}", path.display()))?;
    let first_content = raw
        .lines()
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'));
    let Some(first_content) = first_content else {
        return Ok(Value::Sequence(Vec::new()));
    };
    anyhow::ensure!(
        !first_content.starts_with('%'),
        "DSH profile YAML directives are unsupported: {}",
        path.display()
    );
    // serde_yaml drops expanded extension tags such as !!js. Rebind the global
    // handle while parsing, then restore tags without rewriting !!js text in
    // ordinary strings or inside the expression itself.
    let start = if first_content == "---" || first_content.starts_with("--- #") {
        ""
    } else {
        "---\n"
    };
    let source = format!("%TAG !! !{GLOBAL_TAG_PREFIX}\n{start}{raw}");
    serde_yaml::from_str(&source).with_context(|| format!("parse DSH profile {}", path.display()))
}

fn preserve_global_tags(value: &mut Value, marker: &str) {
    match value {
        Value::Tagged(tagged) => {
            if let Some(name) = tagged
                .tag
                .to_string()
                .strip_prefix(&format!("!{GLOBAL_TAG_PREFIX}"))
            {
                tagged.tag = serde_yaml::value::Tag::new(format!("{marker}{name}"));
            }
            preserve_global_tags(&mut tagged.value, marker);
        }
        Value::Mapping(mapping) => {
            // Config keys are plain strings; values can contain expression nodes.
            for value in mapping.values_mut() {
                preserve_global_tags(value, marker);
            }
        }
        Value::Sequence(sequence) => {
            for value in sequence {
                preserve_global_tags(value, marker);
            }
        }
        _ => {}
    }
}

pub fn sync_client_key(dsh_home: &Path, client_key: &str) -> anyhow::Result<()> {
    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
    set_credential_ref(&mut credentials, API_KEY_ENV, client_key)?;
    let contents = serde_yaml::to_string(&credentials)
        .with_context(|| format!("serialize DSH YAML {}", credentials_path.display()))?;
    write_atomic_if_changed(&credentials_path, contents.as_bytes())?;
    restrict_owner_only_file(&credentials_path)
}

fn read_yaml(path: &Path, label: &str) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(Value::Mapping(Mapping::new()));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read {label} {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Value::Mapping(Mapping::new()));
    }
    serde_yaml::from_str(&raw).with_context(|| format!("parse {label} {}", path.display()))
}

fn write_yaml(path: &Path, value: &Value) -> anyhow::Result<bool> {
    let contents = serde_yaml::to_string(value)
        .with_context(|| format!("serialize DSH YAML {}", path.display()))?;
    let changed = write_atomic_if_changed(path, contents.as_bytes())?;
    restrict_owner_only_file(path)?;
    Ok(changed)
}

fn provider_profile(bind: SocketAddr, models: Vec<Value>) -> Value {
    let mut profile = Mapping::new();
    profile.insert(
        Value::String("api".to_owned()),
        Value::String(API_PROTOCOL.to_owned()),
    );
    profile.insert(
        Value::String("displayName".to_owned()),
        Value::String("Codex Mixin".to_owned()),
    );
    profile.insert(
        Value::String("baseURL".to_owned()),
        Value::String(format!("http://{}/v1", crate::config::client_address(bind))),
    );
    profile.insert(
        Value::String("apiKeyEnv".to_owned()),
        Value::String(API_KEY_ENV.to_owned()),
    );
    profile.insert(Value::String("models".to_owned()), Value::Sequence(models));
    Value::Mapping(profile)
}
