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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_install_sets_coding_detail_mode() {
        for mode in [None, Some("STEPS_PROSE"), Some("STEPS_EXECUTION")] {
            let mut document = "[desktop]\nother = true\n".parse::<DocumentMut>().unwrap();
            if let Some(mode) = mode {
                document["desktop"]["conversationDetailMode"] = value(mode);
            }
            for _ in 0..2 {
                upsert(
                    &mut document,
                    None,
                    Path::new("models.json"),
                    "http://127.0.0.1:8000/v1",
                    "live",
                    None,
                    false,
                )
                .unwrap();
                assert_eq!(
                    document["desktop"]["conversationDetailMode"].as_str(),
                    Some("STEPS_COMMANDS")
                );
                assert_eq!(document["desktop"]["other"].as_bool(), Some(true));
            }
        }
    }

    #[test]
    fn official_install_keeps_suggestion_provider() {
        let mut document = DocumentMut::new();
        upsert(
            &mut document,
            Some("gpt-6-astra"),
            Path::new("models.json"),
            "http://127.0.0.1:8787/v1",
            "live",
            Some("fixture-key"),
            true,
        )
        .unwrap();
        // Desktop suggestions reject any provider id other than openai.
        assert_eq!(document["model_provider"].as_str(), Some("openai"));
        assert_eq!(
            document["openai_base_url"].as_str(),
            Some("http://127.0.0.1:8787/v1")
        );
        assert!(document["model_providers"].get("openai").is_none());
        assert!(document_is_managed(&document.to_string()));
        assert_eq!(
            managed_provider_id(&document).unwrap(),
            crate::CODEX_MIXIN_PROVIDER
        );
    }

    #[test]
    fn official_proxy_preserves_user_openai_table_and_rotates_key() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("config.toml");
        let mut document = "[model_providers.openai]\nhttp_headers = { user = 'keep' }\n"
            .parse::<DocumentMut>()
            .unwrap();
        upsert(
            &mut document,
            None,
            Path::new("models.json"),
            "http://127.0.0.1:8787/v1",
            "live",
            Some("old-key"),
            true,
        )
        .unwrap();
        std::fs::write(&config_path, document.to_string()).unwrap();
        assert!(is_managed(&config_path).unwrap());
        sync_client_key(&config_path, "new-key").unwrap();
        let document = std::fs::read_to_string(&config_path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(
            document["model_providers"]["openai"]["http_headers"]["user"].as_str(),
            Some("keep")
        );
        assert_eq!(
            document["model_providers"][crate::CODEX_MIXIN_PROVIDER]["http_headers"]
                [crate::gateway_access::CODEX_CLIENT_KEY_HEADER]
                .as_str(),
            Some("new-key")
        );
    }

    #[test]
    fn custom_reconnect_clears_official_endpoint_override() {
        let mut document = DocumentMut::new();
        for official in [true, false, true] {
            upsert(
                &mut document,
                None,
                Path::new("models.json"),
                "http://127.0.0.1:8787/v1",
                "live",
                Some("fixture-key"),
                official,
            )
            .unwrap();
            assert_eq!(document.get("openai_base_url").is_some(), official);
            assert_eq!(
                document["model_provider"].as_str(),
                Some(provider_id(official))
            );
            assert!(document_is_managed(&document.to_string()));
        }
    }

    #[test]
    fn plain_openai_proxy_is_not_managed() {
        let document = "model_provider = 'openai'\nopenai_base_url = 'http://127.0.0.1:8787/v1'\nmodel_catalog_json = 'models.json'\n"
            .parse::<DocumentMut>().unwrap();
        assert!(!document_is_managed(&document.to_string()));
        assert!(managed_provider_id(&document).is_err());
    }

    #[test]
    fn official_install_preserves_detail_mode() {
        let mut document = "[desktop]\nconversationDetailMode = 'STEPS_PROSE'\n"
            .parse::<DocumentMut>()
            .unwrap();
        upsert(
            &mut document,
            None,
            Path::new("models.json"),
            "http://127.0.0.1:8000/v1",
            "live",
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            document["desktop"]["conversationDetailMode"].as_str(),
            Some("STEPS_PROSE")
        );
    }

    #[test]
    fn custom_install_rejects_invalid_desktop_settings() {
        let mut document = "desktop = 'invalid'".parse::<DocumentMut>().unwrap();
        let before = document.to_string();
        let error = upsert(
            &mut document,
            None,
            Path::new("models.json"),
            "http://127.0.0.1:8000/v1",
            "live",
            None,
            false,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("desktop settings must be a TOML table")
        );
        assert_eq!(document.to_string(), before);
    }

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
