use std::path::Path;

use toml_edit::{DocumentMut, InlineTable, Item, Table, Value, value};

use super::files::write_atomic_if_changed;

pub const MANAGED_MARKER: &str = "codex-mixin managed config";
pub const MANAGED_HEADER: &str = "# codex-mixin managed config. Run `codex-mixin uninstall-codex` to restore the previous config.";
pub const CUSTOM_ONLY_PROVIDER: &str = "codex-mixin-custom";
/// Older custom-only installs reused the reserved `amazon-bedrock` provider id.
/// Codex >= 0.135 locks that id to `aws.profile`/`aws.region` only and rejects a
/// custom `base_url`, so new installs use `CUSTOM_ONLY_PROVIDER`; this legacy id
/// is still recognized for detection and uninstall of pre-existing installs.
pub const LEGACY_CUSTOM_ONLY_PROVIDER: &str = "amazon-bedrock";

pub fn document_is_managed(raw: &str) -> bool {
    if raw.contains(MANAGED_MARKER) {
        return true;
    }
    // Codex rewrites `config.toml` on run and drops leading comments, which
    // erases MANAGED_HEADER. Fall back to a structural check so install/uninstall
    // keep recognizing our config afterwards: the active `model_provider` points
    // at one of our own provider ids and that provider table is present. Only our
    // dedicated ids are matched (never the reserved `amazon-bedrock`) so a user's
    // real provider is never misdetected as managed.
    document_has_managed_structure(raw)
}

fn document_has_managed_structure(raw: &str) -> bool {
    let Ok(document) = raw.parse::<DocumentMut>() else {
        return false;
    };
    let Some(active) = document.get("model_provider").and_then(Item::as_str) else {
        return false;
    };
    if active != CUSTOM_ONLY_PROVIDER && active != crate::CODEX_MIXIN_PROVIDER {
        return false;
    }
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_some_and(|providers| providers.contains_key(active))
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
    if !codex_oauth_proxy {
        // Custom-only mode authenticates through the managed Bedrock placeholder.
        // A retained ChatGPT/API policy makes Codex ignore that auth document.
        document.remove("forced_login_method");
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
    providers.insert(provider_id, Item::Table(provider));
    Ok(())
}

pub fn provider_id(codex_oauth_proxy: bool) -> &'static str {
    if codex_oauth_proxy {
        crate::CODEX_MIXIN_PROVIDER
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

pub fn managed_provider_id(document: &DocumentMut) -> anyhow::Result<&str> {
    if let Some(provider_id) = document.get("model_provider").and_then(Item::as_str) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_marker_managed_config() {
        let raw = format!("{MANAGED_HEADER}\nmodel = \"x\"\n");
        assert!(document_is_managed(&raw));
    }

    #[test]
    fn detects_managed_config_after_codex_strips_the_comment_header() {
        // Codex rewrites config.toml and drops the leading comment marker; the
        // structural fallback must still recognize our provider so install and
        // uninstall keep working.
        let raw = r#"
model_provider = "codex-mixin-custom"
model_catalog_json = 'C:\Users\me\.codex\model-catalogs\mixin-models.json'
[model_providers.codex-mixin-custom]
name = "Codex Mixin"
base_url = "http://127.0.0.1:8787/v1"
wire_api = "responses"
"#;
        assert!(!raw.contains(MANAGED_MARKER));
        assert!(document_is_managed(raw));
    }

    #[test]
    fn does_not_treat_a_plain_user_config_as_managed() {
        let raw = r#"
model_provider = "oneapi"
[model_providers.oneapi]
name = "oneapi"
base_url = "https://example.com/v1"
"#;
        assert!(!document_is_managed(raw));
    }

    #[test]
    fn does_not_treat_reserved_bedrock_provider_as_managed() {
        let raw = r#"
model_provider = "amazon-bedrock"
[model_providers.amazon-bedrock]
"#;
        assert!(!document_is_managed(raw));
    }
}
