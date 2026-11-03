use std::fs;
use std::path::Path;

const MANAGED_AUTH_KEY_PREFIX: &str = "codex-mixin-local-";

pub(super) fn managed_auth_bytes(contents: Option<&[u8]>) -> anyhow::Result<bool> {
    let Some(contents) = contents else {
        return Ok(false);
    };
    let auth: serde_json::Value = serde_json::from_slice(contents)?;
    let key = match auth.get("auth_mode").and_then(serde_json::Value::as_str) {
        Some("bedrockApiKey") => auth.pointer("/bedrock_api_key/api_key"),
        Some("apikey") => auth.get("OPENAI_API_KEY"),
        _ => None,
    };
    Ok(key
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| key.starts_with(MANAGED_AUTH_KEY_PREFIX)))
}

pub(super) fn managed_apikey_auth() -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(&serde_json::json!({
        "auth_mode": "apikey",
        "OPENAI_API_KEY": format!("{MANAGED_AUTH_KEY_PREFIX}{}", uuid::Uuid::new_v4()),
    }))?)
}

pub(in crate::cli) fn migrate_managed_custom_auth(config_path: &Path) -> anyhow::Result<bool> {
    let auth_path = config_path.with_file_name("auth.json");
    if !auth_path.exists() {
        return Ok(false);
    }
    let contents = fs::read(&auth_path)?;
    if !managed_auth_bytes(Some(&contents))?
        || serde_json::from_slice::<serde_json::Value>(&contents)?["auth_mode"] != "bedrockApiKey"
    {
        return Ok(false);
    }
    let _lock = super::managed_config::ManagedConfigLock::acquire(config_path)?;
    let contents = fs::read(&auth_path)?;
    if !managed_auth_bytes(Some(&contents))?
        || serde_json::from_slice::<serde_json::Value>(&contents)?["auth_mode"] != "bedrockApiKey"
    {
        return Ok(false);
    }
    codex_mixin::platform::write_owner_only(&auth_path, &managed_apikey_auth()?)?;
    Ok(true)
}
