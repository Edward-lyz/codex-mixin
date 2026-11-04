use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::Value as JsonValue;
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
/// Bridge plugin DSH loads to translate Claude Code hooks into DSH events.
pub const REPORT_HOOKS_PLUGIN: &str = "@deepseek-ai/dsh-hooks-claude-code";
/// Peer packages the bridge imports directly. DSH profiles disable pnpm's
/// automatic peer installation, so `dsh plugin add` alone leaves the bridge
/// unable to import and every reporting hook stays inert.
pub const REPORT_HOOKS_PLUGIN_PEERS: [&str; 2] =
    ["@deepseek-ai/dsh-hook-protocol", "@deepseek-ai/dsh-llm"];
const REPORT_HOOKS_ROW_ID: &str = "codex-mixin-report-hooks";

/// One DSH profile whose `cordis.patch.yml` can receive the reporting bridge.
pub struct ReportHookProfile {
    pub name: String,
    pub directory: PathBuf,
    pub patch_path: PathBuf,
}

/// DSH writes one session log per conversation; the model is recorded near the
/// start, so a bounded prefix keeps reporting lookups cheap on long sessions.
const DSH_SESSION_HEAD_BYTES: u64 = 256 * 1024;
const DSH_MODEL_POINTERS: [&str; 5] = [
    "/data/header/config/model",
    "/data/model",
    "/data/route/model",
    "/data/source/model",
    "/data/message/source/model",
];

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

/// Existing profiles that can receive the reporting bridge, in stable order.
pub fn report_hook_profiles(dsh_home: &Path) -> anyhow::Result<Vec<ReportHookProfile>> {
    let profiles = dsh_home.join("profiles");
    let entries = match std::fs::read_dir(&profiles) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read DSH profiles {}", profiles.display()));
        }
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name == "node_modules" || name.to_string_lossy().starts_with('.') {
            continue;
        }
        let patch_path = entry.path().join(PROFILE_PATCH_FILE);
        if patch_path.is_file() {
            found.push(ReportHookProfile {
                name: name.to_string_lossy().into_owned(),
                directory: entry.path(),
                patch_path,
            });
        }
    }
    found.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(found)
}

/// Whether `package` is resolvable from the profile or the shared profiles
/// `node_modules` directory.
pub fn plugin_installed(dsh_home: &Path, profile_dir: &Path, package: &str) -> bool {
    let shared = dsh_home
        .join("profiles")
        .join("node_modules")
        .join(package)
        .join("package.json");
    shared.is_file()
        || profile_dir
            .join("node_modules")
            .join(package)
            .join("package.json")
            .is_file()
}

/// Add the managed reporting bridge row to one profile patch layer.
pub fn mount_report_hooks(patch_path: &Path, hooks_path: &Path) -> anyhow::Result<bool> {
    let mut document = SettingsDocument {
        path: patch_path.to_owned(),
        contents: read_profile(patch_path)?,
        profile: true,
    };
    let entries = document.contents.as_sequence_mut().with_context(|| {
        format!(
            "DSH profile patch must be a YAML sequence: {}",
            patch_path.display()
        )
    })?;
    let (mut retained, _) = without_report_hook_rows(std::mem::take(entries));
    retained.push(report_hook_row(hooks_path));
    *entries = retained;
    document.write()
}

/// Remove the managed reporting bridge row from one profile patch layer.
pub fn unmount_report_hooks(patch_path: &Path) -> anyhow::Result<bool> {
    let mut document = SettingsDocument {
        path: patch_path.to_owned(),
        contents: read_profile(patch_path)?,
        profile: true,
    };
    let entries = document.contents.as_sequence_mut().with_context(|| {
        format!(
            "DSH profile patch must be a YAML sequence: {}",
            patch_path.display()
        )
    })?;
    let (retained, changed) = without_report_hook_rows(std::mem::take(entries));
    *entries = retained;
    if !changed {
        return Ok(false);
    }
    document.write()
}

fn report_hook_row(hooks_path: &Path) -> Value {
    let mut config = Mapping::new();
    config.insert(
        Value::String("configPath".to_owned()),
        Value::String(hooks_path.to_string_lossy().into_owned()),
    );
    let mut row = Mapping::new();
    row.insert(
        Value::String("id".to_owned()),
        Value::String(REPORT_HOOKS_ROW_ID.to_owned()),
    );
    row.insert(
        Value::String("name".to_owned()),
        Value::String(REPORT_HOOKS_PLUGIN.to_owned()),
    );
    row.insert(Value::String("config".to_owned()), Value::Mapping(config));
    let mut insert = Mapping::new();
    insert.insert(
        Value::String("insert".to_owned()),
        Value::Sequence(vec![Value::Mapping(row)]),
    );
    Value::Mapping(insert)
}

fn without_report_hook_rows(mut entries: Vec<Value>) -> (Vec<Value>, bool) {
    let mut changed = false;
    let mut retained = Vec::with_capacity(entries.len());
    for entry in std::mem::take(&mut entries) {
        match strip_report_hook_rows(entry) {
            None => changed = true,
            Some(entry) => retained.push(entry),
        }
    }
    (retained, changed)
}

/// Drop `codex-mixin-report-hooks` rows from one patch entry. Returns `None`
/// when the entry only carried our insert wrapper, so an unrelated `insert`
/// block owned by the user is preserved.
fn strip_report_hook_rows(entry: Value) -> Option<Value> {
    let Some(mapping) = entry.as_mapping() else {
        return Some(entry);
    };
    let key = Value::String("insert".to_owned());
    let Some(rows) = mapping.get(&key).and_then(Value::as_sequence) else {
        return Some(entry);
    };
    if !rows.iter().any(is_report_hook_row) {
        return Some(entry);
    }
    let retained: Vec<Value> = rows
        .iter()
        .filter(|row| !is_report_hook_row(row))
        .cloned()
        .collect();
    if retained.is_empty() {
        return None;
    }
    let mut mapping = mapping.clone();
    mapping.insert(key, Value::Sequence(retained));
    Some(Value::Mapping(mapping))
}

fn is_report_hook_row(row: &Value) -> bool {
    row.get("id").and_then(Value::as_str) == Some(REPORT_HOOKS_ROW_ID)
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

/// DSH hooks run through a bridge whose payload has no model and no usable
/// transcript path, so read the session log the app keeps under `$DSH_HOME`.
pub fn session_model(dsh_home: &Path, session_id: &str) -> anyhow::Result<Option<String>> {
    if let Some(model) = session_log_model(dsh_home, session_id)? {
        return Ok(Some(model));
    }
    projcache_model(dsh_home, session_id)
}

fn session_log_model(dsh_home: &Path, session_id: &str) -> anyhow::Result<Option<String>> {
    let sessions = dsh_home.join("sessions");
    let entries = match std::fs::read_dir(&sessions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("read DSH sessions {}", sessions.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        // DSH may hand the bridge either a bare session UUID or the
        // `session-<uuid>` header id, so accept both directory names.
        for directory in [format!("session-{session_id}"), session_id.to_owned()] {
            let path = entry.path().join(directory).join("session.v4.jsonl.zstd");
            if path.is_file() {
                return zstd_model(&path);
            }
        }
    }
    Ok(None)
}

fn zstd_model(path: &Path) -> anyhow::Result<Option<String>> {
    let file = File::open(path).with_context(|| format!("open DSH session {}", path.display()))?;
    let decoder = zstd::stream::read::Decoder::new(file)
        .with_context(|| format!("decode DSH session {}", path.display()))?;
    let mut head = Vec::new();
    decoder
        .take(DSH_SESSION_HEAD_BYTES)
        .read_to_end(&mut head)
        .with_context(|| format!("read DSH session {}", path.display()))?;
    for line in head.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<JsonValue>(line) else {
            continue;
        };
        for pointer in DSH_MODEL_POINTERS {
            if let Some(model) = value
                .pointer(pointer)
                .and_then(JsonValue::as_str)
                .filter(|model| !model.is_empty())
            {
                return Ok(Some(model.to_owned()));
            }
        }
    }
    Ok(None)
}

fn projcache_model(dsh_home: &Path, session_id: &str) -> anyhow::Result<Option<String>> {
    let directory = dsh_home.join("storages/session_projcache/sessions");
    for name in [
        format!("session-{session_id}.json"),
        format!("{session_id}.json"),
    ] {
        let path = directory.join(name);
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read DSH session cache {}", path.display()));
            }
        };
        let Ok(value) = serde_json::from_slice::<JsonValue>(&raw) else {
            continue;
        };
        if let Some(model) = value
            .pointer("/record/rows/modelSelection/val/lastUsed/model")
            .and_then(JsonValue::as_str)
            .filter(|model| !model.is_empty())
        {
            return Ok(Some(model.to_owned()));
        }
    }
    Ok(None)
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
