use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine;
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

use super::engine;
use crate::clients::IntegrationStatus;

fn directories() -> anyhow::Result<Vec<PathBuf>> {
    let root = crate::platform::framework_app_root()?;
    Ok(vec![root.join("Cindy"), root.join("CindyGlobal")])
}

fn imported(record: &Value, directories: &[PathBuf]) -> anyhow::Result<bool> {
    let id = record["id"]
        .as_str()
        .context("invalid Cindy integration record")?;
    let base = record["base"]
        .as_str()
        .context("invalid Cindy integration record")?;
    let mut unknown = false;
    let mut inspected = 0;
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
            let found = (|| -> anyhow::Result<Option<String>> {
                let connection = rusqlite::Connection::open_with_flags(
                    entry.path(),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                connection.busy_timeout(std::time::Duration::from_millis(500))?;
                Ok(connection
                    .query_row(
                        "SELECT runtimes FROM custom_providers WHERE id=?1 LIMIT 1",
                        [id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?)
            })();
            match found {
                Ok(Some(runtimes)) => {
                    anyhow::ensure!(
                        runtimes.contains(base),
                        "Cindy imported provider was edited; reconcile it in Cindy"
                    );
                    return Ok(true);
                }
                Ok(None) => {}
                Err(_) => unknown = true,
            }
        }
    }
    anyhow::ensure!(
        !unknown,
        "Cindy database could not be inspected; import status is unknown"
    );
    Ok(false)
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

pub(super) fn install(manifest: &Path, base: &str, key: &str) -> anyhow::Result<()> {
    let record = match engine::read_record(manifest)? {
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
        None => json!({"kind":"cindy","id":format!("codex-mixin-{}",uuid::Uuid::new_v4().simple()),
            "base":base,"key":key,"disconnecting":false}),
    };
    engine::write_record(manifest, &record)?;
    if imported(&record, &directories()?)? {
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
    // Revocation is performed by the existing caller after this succeeds.
    record["disconnecting"] = json!(true);
    engine::write_record(manifest, &record)?;
    engine::remove_record(&manifest.with_extension("import.html"))?;
    if !added {
        engine::remove_record(manifest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_and_removal_are_observed_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let roots = vec![directory.path().join("Cindy")];
        std::fs::create_dir_all(&roots[0]).unwrap();
        let database = roots[0].join("cindy-account.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE custom_providers(id TEXT,name TEXT,runtimes TEXT)")
            .unwrap();
        let manifest = directory.path().join("cindy.json");
        let record = json!({"id":"codex-mixin-fixture","base":"http://127.0.0.1:8080","key":"fixture-key","disconnecting":false});
        engine::write_record(&manifest, &record).unwrap();
        let before = std::fs::read(&database).unwrap();
        assert_eq!(status_at(&manifest, &roots).unwrap().state, "pending");
        assert_eq!(std::fs::read(&database).unwrap(), before);
        connection
            .execute(
                "INSERT INTO custom_providers VALUES(?1,'Codex Mixin',?2)",
                rusqlite::params![
                    "codex-mixin-fixture",
                    payload(&record)["endpoints"].to_string()
                ],
            )
            .unwrap();
        assert_eq!(status_at(&manifest, &roots).unwrap().state, "managed");
        let before = std::fs::read(&database).unwrap();
        remove_at(&manifest, &roots).unwrap();
        assert_eq!(
            status_at(&manifest, &roots).unwrap().state,
            "pending_removal"
        );
        assert_eq!(std::fs::read(&database).unwrap(), before);
        connection
            .execute("DELETE FROM custom_providers", [])
            .unwrap();
        assert_eq!(status_at(&manifest, &roots).unwrap().state, "unmanaged");
        remove_at(&manifest, &roots).unwrap();
        assert!(!manifest.exists());
    }

    #[test]
    fn unreadable_schema_is_unknown_instead_of_a_successful_import() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("cindy-broken.db"), b"not sqlite").unwrap();
        let record = json!({"id":"fixture","base":"http://127.0.0.1:8080"});
        assert!(imported(&record, &[directory.path().to_path_buf()]).is_err());
        let value =
            payload(&json!({"id":"fixture","base":"http://127.0.0.1:8080","key":"fixture-key"}));
        assert_eq!(value["endpoints"][0]["protocol"], "anthropic-messages");
        assert_eq!(value["endpoints"][1]["targets"], json!(["codex"]));
        assert_eq!(value["endpoints"][2]["targets"], json!(["pi"]));
    }
}
