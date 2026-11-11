use codex_mixin::config::{load_stored_config, mutate_stored_config};
use codex_mixin::gateway_access::{GatewayApiKey, generate_gateway_api_key};
use serde_json::{Value, json};

use super::args::KeysCommand;

pub(super) fn run(command: KeysCommand) -> anyhow::Result<()> {
    match command {
        KeysCommand::List { json } => {
            let mut config = load_stored_config()?.unwrap_or_default();
            adopt_legacy_key(&mut config.gateway_api_key, &mut config.gateway_api_keys);
            let keys: Vec<Value> = config.gateway_api_keys.iter().map(summary).collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({ "keys": keys }))?
                );
            } else if keys.is_empty() {
                println!("No gateway API keys. Create one with `codex-mixin config keys create`.");
            } else {
                for entry in &config.gateway_api_keys {
                    println!(
                        "{}  {}  {}  {}",
                        entry.id,
                        masked(&entry.key),
                        entry.created_at,
                        entry.name
                    );
                }
            }
        }
        KeysCommand::Create { name, json } => {
            let key = generate_gateway_api_key()?;
            let entry = GatewayApiKey {
                id: key[3..11].to_owned(),
                name: name.trim().to_owned(),
                key,
                created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            };
            anyhow::ensure!(!entry.name.is_empty(), "key name must not be empty");
            mutate_stored_config(|config| {
                adopt_legacy_key(&mut config.gateway_api_key, &mut config.gateway_api_keys);
                config.gateway_api_keys.push(entry.clone());
                Ok(())
            })?;
            if json {
                let mut value = summary(&entry);
                value["key"] = json!(entry.key);
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!(
                    "Created gateway API key {} ({}). It is shown once:\n  {}",
                    entry.id, entry.name, entry.key
                );
            }
        }
        KeysCommand::Revoke { id, json } => {
            let mut removed = false;
            mutate_stored_config(|config| {
                adopt_legacy_key(&mut config.gateway_api_key, &mut config.gateway_api_keys);
                let before = config.gateway_api_keys.len();
                config.gateway_api_keys.retain(|entry| entry.id != id);
                removed = config.gateway_api_keys.len() != before;
                Ok(())
            })?;
            anyhow::ensure!(removed, "no gateway API key with id {id}");
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({ "revoked": id }))?
                );
            } else {
                println!("Revoked gateway API key {id}.");
            }
        }
    }
    Ok(())
}

/// Moves the single key written by earlier versions into the managed list so
/// it can be listed and revoked like any other key.
fn adopt_legacy_key(legacy: &mut Option<String>, keys: &mut Vec<GatewayApiKey>) {
    if let Some(key) = legacy.take()
        && !keys.iter().any(|entry| entry.key == key)
    {
        keys.insert(
            0,
            GatewayApiKey {
                id: key.get(3..11).unwrap_or("default").to_owned(),
                name: "default".to_owned(),
                key,
                created_at: String::new(),
            },
        );
    }
}

fn summary(entry: &GatewayApiKey) -> Value {
    json!({
        "id": entry.id,
        "name": entry.name,
        "masked_key": masked(&entry.key),
        "created_at": entry.created_at,
    })
}

fn masked(key: &str) -> String {
    let tail = key.len().saturating_sub(4);
    format!(
        "{}…{}",
        key.get(..7).unwrap_or(key),
        key.get(tail..).unwrap_or("")
    )
}
