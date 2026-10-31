//! Read-only discovery of supported local AI-coding logins (AGY, Kiro).
//!
//! Discovery never touches the network and never writes the external
//! credential files. A discovered account becomes an ordinary provider built
//! from its preset, tagged with `discovered_credential`.
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use serde_json::Value;

use super::spec::ProviderPreset;
use super::types::ProviderDefinition;

const MAX_CREDENTIAL_BYTES: u64 = 1024 * 1024;
const IGNORE_FILE: &str = "discovery-ignored";
pub(crate) const PENDING_REFRESH: &str = "model directory pending refresh";
const TOKEN_MARGIN_SECONDS: i64 = 180;

/// Presets whose local login Mixin adopts automatically. AGY Business is
/// excluded: a generic Google ADC file does not prove a Business license.
const AUTO_PRESETS: [ProviderPreset; 2] = [ProviderPreset::Agy, ProviderPreset::Kiro];

/// Where the supported logins and the ignore list live; injectable for tests.
#[derive(Clone, Debug)]
pub(crate) struct Locations {
    agy: Option<PathBuf>,
    business: Vec<PathBuf>,
    kiro: PathBuf,
    kiro_cli: Option<PathBuf>,
    ignore: PathBuf,
}

impl Locations {
    pub(crate) fn current() -> Self {
        Self {
            agy: crate::platform::oauth::agy_auth_path(),
            business: std::iter::once(crate::platform::oauth::oauth_path("agy-business"))
                .chain(crate::platform::oauth::agy_external_token_paths())
                .chain(std::iter::once(crate::platform::oauth::google_adc_path()))
                .collect(),
            kiro: crate::platform::oauth::kiro_auth_path(),
            kiro_cli: crate::platform::oauth::kiro_cli_auth_path(),
            ignore: crate::platform::oauth::oauth_path(IGNORE_FILE),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalCredential {
    /// Stable identity of the signed-in account, never the secret itself.
    pub(crate) fingerprint: String,
}

/// Returns the account behind a usable local login, or `None` when the login
/// is absent, malformed or can neither be used nor refreshed.
pub(crate) fn inspect(preset: ProviderPreset, at: &Locations) -> Option<LocalCredential> {
    match preset {
        ProviderPreset::Agy => inspect_agy(at.agy.as_deref()?),
        ProviderPreset::AgyBusiness => {
            // An owned login takes precedence even when broken: explicit
            // authorization must repair it instead of selecting another account.
            let paths = match at.business.first() {
                Some(path) if path.exists() => std::slice::from_ref(path),
                _ => at.business.get(1..)?,
            };
            let document = paths.iter().find_map(|path| {
                let document = read_json(path)?;
                (matches!(
                    text(&document, "auth_method"),
                    Some("gcp" | "business" | "oauth-business")
                ) || (at.business.last() == Some(path)
                    && text(&document, "type") == Some("authorized_user")))
                .then_some(document)
            })?;
            let token = document.get("token").unwrap_or(&document);
            let access = text(token, "access_token");
            let refresh = text(token, "refresh_token").filter(|_| {
                text(token, "client_id").is_some() && text(token, "client_secret").is_some()
            });
            if refresh.is_none() && !(access.is_some() && unexpired(token, "expiry")) {
                return None;
            }
            Some(LocalCredential {
                fingerprint: fingerprint(&["agy-business", refresh.or(access)?]),
            })
        }
        ProviderPreset::Kiro => at
            .kiro_cli
            .as_deref()
            .and_then(|path| {
                let document = super::auth::kiro::cli_credentials(path).ok()??;
                inspect_kiro_document(&document)
            })
            .or_else(|| inspect_kiro(&at.kiro)),
        _ => None,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut raw = Vec::new();
    file.take(MAX_CREDENTIAL_BYTES + 1)
        .read_to_end(&mut raw)
        .ok()?;
    if raw.len() as u64 > MAX_CREDENTIAL_BYTES {
        return None;
    }
    serde_json::from_slice(&raw).ok()
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

fn unexpired(value: &Value, key: &str) -> bool {
    text(value, key)
        .and_then(|expiry| chrono::DateTime::parse_from_rfc3339(expiry).ok())
        .is_some_and(|expiry| {
            expiry.timestamp() > chrono::Utc::now().timestamp() + TOKEN_MARGIN_SECONDS
        })
}

fn fingerprint(parts: &[&str]) -> String {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    for part in parts {
        context.update(part.as_bytes());
        context.update(&[0]);
    }
    context.finish().as_ref()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn inspect_agy(path: &Path) -> Option<LocalCredential> {
    let document = read_json(path)?;
    // Business sessions use a separate preset and header set.
    if matches!(
        text(&document, "auth_method"),
        Some("gcp" | "business" | "oauth-business")
    ) {
        return None;
    }
    let token = document.get("token")?;
    let refresh = text(token, "refresh_token");
    let access = text(token, "access_token");
    if refresh.is_none() && !(access.is_some() && unexpired(token, "expiry")) {
        return None;
    }
    Some(LocalCredential {
        fingerprint: fingerprint(&["agy", refresh.or(access)?]),
    })
}

fn inspect_kiro(path: &Path) -> Option<LocalCredential> {
    let document = read_json(path)?;
    inspect_kiro_document(&document)
}

fn inspect_kiro_document(document: &Value) -> Option<LocalCredential> {
    let refresh = text(document, "refreshToken");
    let access = text(document, "accessToken");
    if refresh.is_none() && !(access.is_some() && unexpired(document, "expiresAt")) {
        return None;
    }
    // The refresh token rotates when Kiro renews the session, so identify the
    // account by profile or client registration instead.
    let identity = text(document, "profileArn")
        .or_else(|| text(document, "clientIdHash"))
        .or_else(|| text(document, "sourceKey"))
        .or_else(|| text(document, "provider"))
        .unwrap_or("default");
    Some(LocalCredential {
        fingerprint: fingerprint(&["kiro", identity]),
    })
}

fn ignored_at(at: &Locations) -> Vec<String> {
    std::fs::read(&at.ignore)
        .ok()
        .and_then(|raw| serde_json::from_slice::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

/// Remember (or forget) that the user removed an auto-discovered account.
pub fn set_preset_ignored(preset_id: &str, ignored: bool) -> anyhow::Result<()> {
    set_ignored_at(&Locations::current(), preset_id, ignored)
}

fn set_ignored_at(at: &Locations, preset_id: &str, ignored: bool) -> anyhow::Result<()> {
    let mut entries = ignored_at(at);
    let present = entries.iter().any(|entry| entry == preset_id);
    if present == ignored {
        return Ok(());
    }
    if ignored {
        entries.push(preset_id.to_owned());
    } else {
        entries.retain(|entry| entry != preset_id);
    }
    crate::platform::oauth::save_oauth(&at.ignore, &serde_json::to_vec(&entries)?)
        .context("save ignored discovered providers")
}

/// Providers found in local logins that no stored provider already covers.
/// A configured provider of the same preset wins, even when disabled.
pub fn discover(configured: &[ProviderDefinition]) -> anyhow::Result<Vec<ProviderDefinition>> {
    discover_at(&Locations::current(), configured)
}

fn discover_at(
    at: &Locations,
    configured: &[ProviderDefinition],
) -> anyhow::Result<Vec<ProviderDefinition>> {
    let ignored = ignored_at(at);
    let mut found = Vec::new();
    for preset in AUTO_PRESETS {
        let preset_id = preset.as_str();
        if configured
            .iter()
            .any(|provider| provider.preset_id.as_deref() == Some(preset_id))
            || ignored.iter().any(|entry| entry == preset_id)
        {
            continue;
        }
        let Some(credential) = inspect(preset, at) else {
            continue;
        };
        let id = preset.default_id();
        ensure!(
            !configured.iter().any(|provider| provider.id == id),
            "cannot add the discovered {preset_id} login: provider id {id} is already used by a different provider"
        );
        let mut provider = preset.create(id, "");
        provider.discovered_credential = Some(credential.fingerprint);
        provider.models_refresh_error = Some(PENDING_REFRESH.to_owned());
        found.push(provider);
    }
    Ok(found)
}

/// Applies discovery to a stored configuration: adds new accounts and resets
/// the model directory of an auto-discovered provider whose account changed.
/// Returns whether anything changed.
pub fn reconcile(providers: &mut Vec<ProviderDefinition>) -> anyhow::Result<bool> {
    reconcile_at(&Locations::current(), providers)
}

fn reconcile_at(at: &Locations, providers: &mut Vec<ProviderDefinition>) -> anyhow::Result<bool> {
    let mut changed = false;
    for provider in providers.iter_mut() {
        let Some(previous) = provider.discovered_credential.clone() else {
            continue;
        };
        let Some(preset) = provider
            .preset_id
            .as_deref()
            .and_then(|id| ProviderPreset::parse(id).ok())
        else {
            continue;
        };
        let Some(current) = inspect(preset, at) else {
            continue;
        };
        if current.fingerprint != previous {
            provider.discovered_credential = Some(current.fingerprint);
            provider.cached_models.clear();
            provider.selected_models.clear();
            provider.new_models.clear();
            provider.models_refreshed_at_ms = None;
            provider.models_refresh_error = Some(PENDING_REFRESH.to_owned());
            changed = true;
        }
    }
    let added = discover_at(at, providers)?;
    changed |= !added.is_empty();
    providers.extend(added);
    Ok(changed)
}

/// Runtime view of the stored providers: persisted auto-discovered accounts
/// whose login disappeared are dropped, and unsaved discoveries are added.
pub fn runtime_providers(
    stored: Vec<ProviderDefinition>,
) -> anyhow::Result<Vec<ProviderDefinition>> {
    runtime_providers_at(&Locations::current(), stored)
}

fn runtime_providers_at(
    at: &Locations,
    stored: Vec<ProviderDefinition>,
) -> anyhow::Result<Vec<ProviderDefinition>> {
    let mut providers: Vec<ProviderDefinition> = stored
        .into_iter()
        .filter(|provider| {
            provider.discovered_credential.is_none()
                || provider
                    .preset_id
                    .as_deref()
                    .and_then(|id| ProviderPreset::parse(id).ok())
                    .and_then(|preset| inspect(preset, at))
                    .is_some()
        })
        .collect();
    providers.extend(discover_at(at, &providers)?);
    Ok(providers)
}
