use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine;
use serde_json::{Value, json};

use super::engine;
use crate::clients::IntegrationStatus;

fn directories() -> anyhow::Result<Vec<PathBuf>> {
    let root = crate::platform::framework_app_root()?;
    Ok(vec![root.join("Cindy"), root.join("CindyGlobal")])
}

fn verify_runtimes(runtimes: &str, base: &str) -> anyhow::Result<()> {
    let runtimes: Value = serde_json::from_str(runtimes)
        .context("Cindy imported provider runtime metadata is invalid")?;
    anyhow::ensure!(
        runtimes
            .as_object()
            .is_some_and(|runtimes| runtimes.len() == 3),
        "Cindy imported provider was edited; reconcile it in Cindy"
    );
    for (agent, protocol) in [
        ("claude-code", "anthropic-messages"),
        ("codex", "openai-responses"),
        ("pi", "openai-chat"),
    ] {
        let expected_base = if agent == "claude-code" {
            base.to_owned()
        } else {
            format!("{base}/v1")
        };
        let runtime = &runtimes[agent];
        anyhow::ensure!(
            runtime["wireProtocol"] == protocol
                && runtime["baseUrl"] == expected_base
                && runtime["modelsUrl"] == format!("{base}/v1/models")
                && runtime.get("requestPath").is_none_or(Value::is_null),
            "Cindy imported provider was edited; reconcile it in Cindy"
        );
    }
    Ok(())
}

fn imported(record: &Value, directories: &[PathBuf]) -> anyhow::Result<bool> {
    let id = record["id"]
        .as_str()
        .context("invalid Cindy integration record")?;
    let legacy = id.strip_prefix("codex-mixin-").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    anyhow::ensure!(
        !id.is_empty()
            && (id.len() <= 40 || legacy)
            && id.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || b"_-".contains(&byte)),
        "invalid Cindy import identifier"
    );
    let base = record["base"]
        .as_str()
        .context("invalid Cindy integration record")?;
    // Native imports preserve at most 24 slug characters, then append eight
    // random hexadecimal characters. The original slug is not the native ID.
    let slug: String = id.chars().take(24).collect();
    let pattern = format!("{slug}-{}", "[0-9a-f]".repeat(8));
    let mut unknown = false;
    let mut inspected = 0;
    let mut matches = 0;
    for directory in directories {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                unknown = true;
                continue;
            }
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("cindy-") || !name.ends_with(".db") {
                continue;
            }
            inspected += 1;
            anyhow::ensure!(inspected <= 128, "too many Cindy account databases");
            if !entry.file_type()?.is_file() {
                unknown = true;
                continue;
            }
            let found = (|| -> anyhow::Result<Vec<String>> {
                let connection = rusqlite::Connection::open_with_flags(
                    entry.path(),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                connection.busy_timeout(std::time::Duration::from_millis(500))?;
                let mut statement = connection.prepare(
                    "SELECT runtimes FROM custom_providers WHERE id=?1 OR id GLOB ?2 LIMIT 2",
                )?;
                let rows = statement.query_map(rusqlite::params![id, pattern], |row| {
                    row.get::<_, String>(0)
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })();
            match found {
                Ok(found) => {
                    matches += found.len();
                    anyhow::ensure!(
                        matches <= 1,
                        "multiple Cindy imports match this integration; reconcile them in Cindy"
                    );
                    for runtimes in found {
                        verify_runtimes(&runtimes, base)?;
                    }
                }
                Err(_) => unknown = true,
            }
        }
    }
    anyhow::ensure!(
        !unknown,
        "Cindy database could not be inspected; import status is unknown"
    );
    Ok(matches == 1)
}

pub(super) fn status(manifest: &Path) -> anyhow::Result<IntegrationStatus> {
    status_at(manifest, &directories()?)
}

fn status_at(manifest: &Path, directories: &[PathBuf]) -> anyhow::Result<IntegrationStatus> {
    let Some(record) = engine::read_record(manifest)? else {
        return Ok(IntegrationStatus {
            state: "unmanaged",
            mode: Some("confirmed_import"),
            detail: "Cindy confirms provider imports in its own application.",
            error: None,
        });
    };
    let added = imported(&record, directories)?;
    let removing = record["disconnecting"] == true;
    Ok(IntegrationStatus {
        state: if removing {
            if added {
                "pending_removal"
            } else {
                "unmanaged"
            }
        } else if added {
            "managed"
        } else {
            "pending"
        },
        mode: Some("confirmed_import"),
        detail: if removing && added {
            "Gateway access is revoked. Remove the imported Codex Mixin provider in Cindy, then disconnect again to clear its record."
        } else if removing {
            "The imported provider has been removed from Cindy."
        } else if added {
            "Cindy confirmed the imported provider in its native database."
        } else {
            "Open the private import page and confirm the provider in Cindy. Its encrypted database is never edited externally."
        },
        error: None,
    })
}

fn payload(record: &Value) -> Value {
    let base = record["base"].as_str().unwrap_or("");
    let v1 = format!("{base}/v1");
    let models = format!("{v1}/models");
    json!({"kind":"custom","name":"Codex Mixin","id":record["id"],
    "auth":{"method":"apiKey","apiKey":record["key"]},
    "endpoints":[
        {"protocol":"anthropic-messages","baseUrl":base,"targets":["claude-code"],"modelsUrl":models},
        {"protocol":"openai-responses","baseUrl":v1,"targets":["codex"],"modelsUrl":models},
        {"protocol":"openai-chat","baseUrl":v1,"targets":["pi"],"modelsUrl":models}
    ]})
}

fn prepare_import(
    manifest: &Path,
    base: &str,
    key: &str,
    directories: &[PathBuf],
) -> anyhow::Result<(Value, bool)> {
    let mut record = match engine::read_record(manifest)? {
        Some(record) => {
            anyhow::ensure!(
                record["disconnecting"] != true,
                "finish removing the previous Cindy import before reconnecting"
            );
            anyhow::ensure!(
                record["base"] == base && record["key"] == key,
                "Cindy binding changed; remove the previous import before reconnecting"
            );
            record
        }
        None => {
            json!({"kind":"cindy","id":format!("cmx-{:020x}",uuid::Uuid::new_v4().as_u128() >> 48),
            "base":base,"key":key,"disconnecting":false})
        }
    };
    let added = imported(&record, directories)?;
    if !added && let Some(id) = record["id"].as_str().filter(|id| id.len() > 40) {
        // Previous releases generated 44-character IDs, which Cindy rejects.
        // Keep their random prefix and binding so retries cannot create a new
        // identity or rotate a credential belonging to an existing import.
        record["id"] = json!(id.chars().take(24).collect::<String>());
    }
    engine::write_record(manifest, &record)?;
    if added {
        engine::remove_record(&manifest.with_extension("import.html"))?;
    }
    Ok((record, added))
}

pub(super) fn install(manifest: &Path, base: &str, key: &str) -> anyhow::Result<()> {
    let (record, added) = prepare_import(manifest, base, key, &directories()?)?;
    if added {
        return Ok(());
    }
    let uri = format!(
        "cindy://provider/import?v=1&data={}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload(&record))?)
    );
    crate::platform::agent_cli::open_cindy_import(&manifest.with_extension("import.html"), &uri)
}

pub(super) fn remove(manifest: &Path) -> anyhow::Result<()> {
    remove_at(manifest, &directories()?)
}

fn remove_at(manifest: &Path, directories: &[PathBuf]) -> anyhow::Result<()> {
    let Some(mut record) = engine::read_record(manifest)? else {
        return Ok(());
    };
    let added = imported(&record, directories)?;
    // The CLI revokes access before native cleanup can remain pending.
    record["disconnecting"] = json!(true);
    engine::write_record(manifest, &record)?;
    engine::remove_record(&manifest.with_extension("import.html"))?;
    if !added {
        engine::remove_record(manifest)?;
    }
    Ok(())
}
