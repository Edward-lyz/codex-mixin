use std::path::Path;

use toml_edit::{DocumentMut, InlineTable, Item, Table, Value, value};

use crate::platform::write_atomic_if_changed;

pub(crate) mod titles;

pub const MANAGED_MARKER: &str = "codex-mixin managed config";
pub const MANAGED_HEADER: &str = "# codex-mixin managed config. Run `codex-mixin uninstall-codex` to restore the previous config.";
pub const CUSTOM_ONLY_PROVIDER: &str = "codex-mixin-custom";
const CODING_DETAIL_MODE: &str = "STEPS_COMMANDS";
/// Older custom-only installs reused the reserved `amazon-bedrock` provider id.
/// Codex >= 0.135 locks that id to `aws.profile`/`aws.region` only and rejects a
/// custom `base_url`, so new installs use `CUSTOM_ONLY_PROVIDER`; this legacy id
/// is still recognized for detection and uninstall of pre-existing installs.
pub const LEGACY_CUSTOM_ONLY_PROVIDER: &str = "amazon-bedrock";

pub fn document_is_managed(raw: &str) -> bool {
    if raw.contains(MANAGED_MARKER) {
        return true;
    }
    // Codex can strip comments. Recognize only Mixin-owned provider tables;
    // an ordinary OpenAI proxy configuration must never be claimed as managed.
    raw.parse::<DocumentMut>()
        .is_ok_and(|document| document_has_managed_structure(&document))
}

fn document_has_managed_structure(document: &DocumentMut) -> bool {
    let Some(active) = document.get("model_provider").and_then(Item::as_str) else {
        return false;
    };
    let provider_table = if active == provider_id(true)
        && document
            .get("openai_base_url")
            .and_then(Item::as_str)
            .is_some()
        && document
            .get("model_catalog_json")
            .and_then(Item::as_str)
            .is_some()
    {
        crate::CODEX_MIXIN_PROVIDER
    } else if active == CUSTOM_ONLY_PROVIDER || active == crate::CODEX_MIXIN_PROVIDER {
        active
    } else {
        return false;
    };
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_some_and(|providers| providers.contains_key(provider_table))
}

pub fn serialize(document: &DocumentMut) -> String {
    let raw = document.to_string();
    let mut serialized = String::with_capacity(MANAGED_HEADER.len() + 1 + raw.len());
    serialized.push_str(MANAGED_HEADER);
    serialized.push('\n');
    let mut in_preamble = true;
    for line in raw.split_inclusive('\n') {
        let trimmed = line.trim();
        if in_preamble && !trimmed.is_empty() && !trimmed.starts_with('#') {
            in_preamble = false;
        }
        if in_preamble && trimmed == MANAGED_HEADER {
            continue;
        }
        serialized.push_str(line);
    }
    serialized
}

pub fn upsert(
    document: &mut DocumentMut,
    default_model: Option<&str>,
    catalog_path: &Path,
    base_url: &str,
    web_search: &str,
    client_key: Option<&str>,
    codex_oauth_proxy: bool,
) -> anyhow::Result<()> {
    let provider_id = provider_id(codex_oauth_proxy);
    if codex_oauth_proxy {
        // Desktop suggestions require the built-in OpenAI identity. Codex
        // ignores model_providers.openai; its supported endpoint override is here.
        document["openai_base_url"] = value(base_url);
    }
    if !codex_oauth_proxy {
        if document
            .get("desktop")
            .is_some_and(|desktop| !desktop.is_table_like())
        {
            anyhow::bail!("desktop settings must be a TOML table");
        }
        // Prose selects ChatGPT Work in current desktop builds; custom auth
        // needs the coding surface. Keep this in the managed config restore point.
        document["desktop"]["conversationDetailMode"] = value(CODING_DETAIL_MODE);
        // Custom-only mode authenticates through the managed Bedrock placeholder.
        // A retained ChatGPT/API policy makes Codex ignore that auth document.
        document.remove("forced_login_method");
        document.remove("openai_base_url");
    }
    document["model_catalog_json"] = value(catalog_path.to_string_lossy().to_string());
    document["model_provider"] = value(provider_id);
    document["web_search"] = value(web_search);
    if let Some(model) = default_model {
        document["model"] = value(model);
    }
    if !document.get("model_providers").is_some_and(Item::is_table) {
        document["model_providers"] = Item::Table(Table::new());
    }
    let providers = document["model_providers"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("model_providers must be a TOML table"))?;
    // Remove only the managed provider ids we own. The legacy custom-only id
    // (`amazon-bedrock`) is intentionally left untouched here because it may also
    // be a user's real Amazon Bedrock provider; legacy managed installs are
    // cleaned up through uninstall instead.
    providers.remove(crate::CODEX_MIXIN_PROVIDER);
    providers.remove(CUSTOM_ONLY_PROVIDER);
    let mut provider = Table::new();
    provider["name"] = value("Codex Mixin");
    provider["base_url"] = value(base_url);
    provider["wire_api"] = value("responses");
    if let Some(client_key) = client_key {
        set_client_key_header(&mut provider, client_key)?;
    }
    if codex_oauth_proxy {
        provider["requires_openai_auth"] = value(true);
        provider["supports_websockets"] = value(true);
    }
    // In official mode this inactive table retains Mixin ownership and key
    // metadata across Codex comment rewrites; it does not override OpenAI.
    let provider_table = if codex_oauth_proxy {
        crate::CODEX_MIXIN_PROVIDER
    } else {
        provider_id
    };
    providers.insert(provider_table, Item::Table(provider));
    Ok(())
}

pub fn provider_id(codex_oauth_proxy: bool) -> &'static str {
    if codex_oauth_proxy {
        "openai"
    } else {
        CUSTOM_ONLY_PROVIDER
    }
}

pub fn is_managed(config_path: &Path) -> anyhow::Result<bool> {
    if !config_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(config_path)?;
    if !document_is_managed(&raw) {
        return Ok(false);
    }
    let document = raw.parse::<DocumentMut>()?;
    managed_provider_id(&document)?;
    Ok(true)
}

pub fn sync_client_key(config_path: &Path, client_key: &str) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(config_path)?;
    let mut document = raw.parse::<DocumentMut>()?;
    let provider_id = managed_provider_id(&document)?.to_owned();
    let provider = document
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .and_then(|providers| providers.get_mut(&provider_id))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("managed Codex provider table is missing"))?;
    set_client_key_header(provider, client_key)?;
    let mut serialized = document.to_string();
    if !serialized.ends_with(char::from(10)) {
        serialized.push(char::from(10));
    }
    write_atomic_if_changed(config_path, serialized.as_bytes())?;
    Ok(())
}

/// Return the Mixin-owned table id, which is inactive with built-in OpenAI.
pub fn managed_provider_id(document: &DocumentMut) -> anyhow::Result<&str> {
    if let Some(provider_id) = document.get("model_provider").and_then(Item::as_str) {
        if provider_id == self::provider_id(true) && document_has_managed_structure(document) {
            return Ok(crate::CODEX_MIXIN_PROVIDER);
        }
        if provider_id == crate::CODEX_MIXIN_PROVIDER
            || provider_id == CUSTOM_ONLY_PROVIDER
            || provider_id == LEGACY_CUSTOM_ONLY_PROVIDER
        {
            return Ok(provider_id);
        }
        anyhow::bail!("unsupported managed Codex provider: {provider_id}");
    }
    let providers = document.get("model_providers").and_then(Item::as_table);
    for candidate in [
        CUSTOM_ONLY_PROVIDER,
        LEGACY_CUSTOM_ONLY_PROVIDER,
        crate::CODEX_MIXIN_PROVIDER,
    ] {
        if providers.is_some_and(|providers| providers.contains_key(candidate)) {
            return Ok(candidate);
        }
    }
    anyhow::bail!("managed Codex config has no supported provider table")
}

fn set_client_key_header(provider: &mut Table, client_key: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !client_key.trim().is_empty(),
        "Codex client key must not be empty"
    );
    anyhow::ensure!(
        client_key == client_key.trim(),
        "Codex client key has whitespace"
    );
    let headers = provider
        .entry("http_headers")
        .or_insert(Item::Value(Value::InlineTable(InlineTable::new())))
        .as_inline_table_mut()
        .ok_or_else(|| anyhow::anyhow!("managed Codex http_headers must be an inline table"))?;
    headers.insert(
        crate::gateway_access::CODEX_CLIENT_KEY_HEADER,
        Value::from(client_key),
    );
    Ok(())
}
