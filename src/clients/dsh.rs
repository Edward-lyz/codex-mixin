use std::net::SocketAddr;
use std::path::Path;

use anyhow::Context;
use serde_yaml::{Mapping, Value};

use super::files::{ensure_owner_only_dir, set_owner_only, write_atomic_if_changed};

const PROVIDER_ID: &str = "codex-mixin";
const API_KEY_ENV: &str = "CODEX_MIXIN_GATEWAY_API_KEY";
const API_PROTOCOL: &str = "openai-responses";
const CREDENTIALS_VERSION_FIELD: &str = "version";
const CREDENTIALS_REFS_FIELD: &str = "refs";
const CREDENTIALS_RECORDS_FIELD: &str = "records";

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
    let settings_path = dsh_home.join("settings.yaml");
    let mut settings = read_yaml(&settings_path, "DSH settings")?;
    let root = settings
        .as_mapping_mut()
        .context("DSH settings must be a YAML mapping")?;
    let llm_pi_ai = root
        .entry(Value::String("llm-pi-ai".to_owned()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .context("DSH settings llm-pi-ai must be a YAML mapping")?;
    let providers = llm_pi_ai
        .entry(Value::String("providers".to_owned()))
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .context("DSH settings llm-pi-ai.providers must be a YAML mapping")?;
    providers.insert(
        Value::String(PROVIDER_ID.to_owned()),
        provider_profile(bind, models),
    );

    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
    set_credential_ref(&mut credentials, API_KEY_ENV, client_key)?;
    let credentials_changed = write_yaml(&credentials_path, &credentials)?;
    let settings_changed = write_yaml(&settings_path, &settings)?;
    Ok(credentials_changed || settings_changed)
}

pub fn uninstall(dsh_home: &Path) -> anyhow::Result<()> {
    let settings_path = dsh_home.join("settings.yaml");
    anyhow::ensure!(
        settings_path.exists(),
        "DSH settings are not managed by codex-mixin: {}",
        settings_path.display()
    );
    let mut settings = read_yaml(&settings_path, "DSH settings")?;
    let root = settings
        .as_mapping_mut()
        .context("DSH settings must be a YAML mapping")?;
    let llm_pi_ai = root
        .get_mut(Value::String("llm-pi-ai".to_owned()))
        .and_then(Value::as_mapping_mut)
        .context("DSH settings llm-pi-ai must be a YAML mapping")?;
    let providers = llm_pi_ai
        .get_mut(Value::String("providers".to_owned()))
        .and_then(Value::as_mapping_mut)
        .context("DSH settings llm-pi-ai.providers must be a YAML mapping")?;
    let profile = providers
        .remove(Value::String(PROVIDER_ID.to_owned()))
        .context("DSH provider codex-mixin is not installed")?;

    if profile.get("apiKeyEnv").and_then(Value::as_str) == Some(API_KEY_ENV) {
        let credentials_path = dsh_home.join(".credentials.yaml");
        if credentials_path.exists() {
            let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
            remove_credential_ref(&mut credentials, API_KEY_ENV)?;
            write_yaml(&credentials_path, &credentials)?;
        }
    }
    if providers.is_empty() {
        llm_pi_ai.remove(Value::String("providers".to_owned()));
    }
    if llm_pi_ai.is_empty() {
        root.remove(Value::String("llm-pi-ai".to_owned()));
    }
    if root.is_empty() {
        std::fs::remove_file(&settings_path)
            .with_context(|| format!("remove empty DSH settings {}", settings_path.display()))?;
    } else {
        write_yaml(&settings_path, &settings)?;
    }
    Ok(())
}

pub fn is_managed(dsh_home: &Path) -> anyhow::Result<bool> {
    let settings_path = dsh_home.join("settings.yaml");
    if !settings_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(&settings_path)?;
    if !raw.contains("displayName: Codex Mixin") {
        return Ok(false);
    }
    let settings = read_yaml(&settings_path, "DSH settings")?;
    Ok(settings
        .get("llm-pi-ai")
        .and_then(|value| value.get("providers"))
        .and_then(|value| value.get(PROVIDER_ID))
        .and_then(|provider| provider.get("displayName"))
        .and_then(Value::as_str)
        == Some("Codex Mixin"))
}

pub fn sync_client_key(dsh_home: &Path, client_key: &str) -> anyhow::Result<()> {
    let credentials_path = dsh_home.join(".credentials.yaml");
    let mut credentials = read_yaml(&credentials_path, "DSH credentials")?;
    set_credential_ref(&mut credentials, API_KEY_ENV, client_key)?;
    let contents = serde_yaml::to_string(&credentials)
        .with_context(|| format!("serialize DSH YAML {}", credentials_path.display()))?;
    write_atomic_if_changed(&credentials_path, contents.as_bytes())?;
    set_owner_only(&credentials_path)
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
    set_owner_only(path)?;
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
        Value::String(format!("http://{bind}/v1")),
    );
    profile.insert(
        Value::String("apiKeyEnv".to_owned()),
        Value::String(API_KEY_ENV.to_owned()),
    );
    profile.insert(Value::String("models".to_owned()), Value::Sequence(models));
    Value::Mapping(profile)
}
