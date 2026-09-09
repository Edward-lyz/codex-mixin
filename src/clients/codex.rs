use std::path::Path;

use toml_edit::{DocumentMut, InlineTable, Item, Value};

use super::files::write_atomic_if_changed;

const MANAGED_MARKER: &str = "codex-mixin managed config";
const CUSTOM_ONLY_PROVIDER: &str = "amazon-bedrock";

pub fn is_managed(config_path: &Path) -> anyhow::Result<bool> {
    if !config_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(config_path)?;
    if !raw.contains(MANAGED_MARKER) {
        return Ok(false);
    }
    let document = raw.parse::<DocumentMut>()?;
    managed_provider_id(&document)?;
    Ok(true)
}

pub fn sync_client_key(config_path: &Path, client_key: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !client_key.trim().is_empty(),
        "Codex client key must not be empty"
    );
    anyhow::ensure!(
        client_key == client_key.trim(),
        "Codex client key has whitespace"
    );
    let raw = std::fs::read_to_string(config_path)?;
    let mut document = raw.parse::<DocumentMut>()?;
    let provider_id = managed_provider_id(&document)?.to_owned();
    let provider = document
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .and_then(|providers| providers.get_mut(&provider_id))
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("managed Codex provider table is missing"))?;
    let headers = provider
        .entry("http_headers")
        .or_insert(Item::Value(Value::InlineTable(InlineTable::new())))
        .as_inline_table_mut()
        .ok_or_else(|| anyhow::anyhow!("managed Codex http_headers must be an inline table"))?;
    headers.insert(
        crate::gateway_access::CODEX_CLIENT_KEY_HEADER,
        Value::from(client_key),
    );
    let mut serialized = document.to_string();
    if !serialized.ends_with(char::from(10)) {
        serialized.push(char::from(10));
    }
    write_atomic_if_changed(config_path, serialized.as_bytes())?;
    Ok(())
}

fn managed_provider_id(document: &DocumentMut) -> anyhow::Result<&str> {
    if let Some(provider_id) = document.get("model_provider").and_then(Item::as_str) {
        if provider_id == crate::CODEX_MIXIN_PROVIDER || provider_id == CUSTOM_ONLY_PROVIDER {
            return Ok(provider_id);
        }
        anyhow::bail!("unsupported managed Codex provider: {provider_id}");
    }
    let providers = document.get("model_providers").and_then(Item::as_table);
    if providers.is_some_and(|providers| providers.contains_key(CUSTOM_ONLY_PROVIDER)) {
        return Ok(CUSTOM_ONLY_PROVIDER);
    }
    if providers.is_some_and(|providers| providers.contains_key(crate::CODEX_MIXIN_PROVIDER)) {
        return Ok(crate::CODEX_MIXIN_PROVIDER);
    }
    anyhow::bail!("managed Codex config has no supported provider table")
}
