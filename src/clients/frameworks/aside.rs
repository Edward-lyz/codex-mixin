use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Value, json};

use super::{FrameworkModel, PROVIDER, engine, field};
use crate::clients::IntegrationStatus;

type RuntimeCall = fn(&str, Option<&Value>) -> anyhow::Result<Value>;

fn account_directory(root: &Path) -> anyhow::Result<(String, PathBuf)> {
    let accounts = engine::read_config(&root.join("accounts.json"), engine::Format::Json)?;
    let raw = accounts.get("currentAccountId");
    let number = match raw {
        Some(Value::String(value)) => value
            .trim()
            .strip_prefix('u')
            .unwrap_or(value.trim())
            .parse::<u64>()?,
        Some(value) => value.as_u64().context("invalid Aside account id")?,
        None => 0,
    };
    let directory = root.join("u").join(number.to_string());
    anyhow::ensure!(
        directory.is_dir(),
        "open Aside and create/select an account before connecting"
    );
    Ok((format!("u{number}"), directory))
}

pub(super) fn install(
    manifest: &Path,
    base: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<()> {
    let root = super::home_path("", ".aside")?;
    install_at(
        manifest,
        &root,
        (base, key),
        models,
        crate::platform::agent_cli::aside_default_model,
    )
}

fn install_at(
    manifest: &Path,
    root: &Path,
    binding: (&str, &str),
    models: &[FrameworkModel],
    runtime: RuntimeCall,
) -> anyhow::Result<()> {
    let first = models
        .first()
        .context("no selected models are available for Aside")?;
    let record_path = manifest.with_extension("aside.json");
    let mut record = match engine::read_record(&record_path)? {
        Some(record) => record,
        None => {
            let (account, directory) = account_directory(root)?;
            let settings =
                engine::read_config(&directory.join("settings.json"), engine::Format::Json)?;
            let before = match runtime(&account, None) {
                Ok(before) => before,
                Err(_) => settings.get("defaultModel").cloned().unwrap_or(Value::Null),
            };
            let mut after = before
                .as_object()
                .cloned()
                .map(Value::Object)
                .unwrap_or_else(|| json!({}));
            after["provider"] = json!(PROVIDER);
            after["modelId"] = json!(first.id);
            if after.get("thinkingLevel").is_none() {
                after["thinkingLevel"] = json!("medium");
            }
            json!({"account":account,"directory":directory,"before":before,"after":after,"phase":"staged"})
        }
    };
    let directory = PathBuf::from(
        record["directory"]
            .as_str()
            .context("invalid Aside record")?,
    );
    let account = record["account"]
        .as_str()
        .context("invalid Aside record")?
        .to_owned();
    engine::write_record(&record_path, &record)?;
    engine::apply(
        manifest,
        vec![
            engine::Patch {
                path: directory.join("models.json"),
                format: engine::Format::Json,
                fields: vec![(
                    vec!["providers".into(), PROVIDER.into()],
                    json!({"baseUrl":format!("{}/v1",binding.0),"api":"openai-completions",
                "apiKey":binding.1,"models":super::pi_models(models,&format!("{}/v1",binding.0))}),
                )],
            },
            engine::Patch {
                path: directory.join("settings.json"),
                format: engine::Format::Json,
                fields: vec![field(&["defaultModel"], record["after"].clone())],
            },
        ],
    )?;
    match runtime(&account, Some(&record["after"])) {
        Ok(actual) => {
            anyhow::ensure!(
                actual == record["after"],
                "Aside did not confirm the requested model"
            );
            record["phase"] = json!("managed");
            record["runtime_error"] = Value::Null;
        }
        Err(_) => {
            record["phase"] = json!("staged");
            record["runtime_error"] =
                json!("Aside runtime could not confirm the model; restart Aside for this account.");
        }
    }
    engine::write_record(&record_path, &record)
}

pub(super) fn status(manifest: &Path) -> anyhow::Result<IntegrationStatus> {
    status_with(manifest, crate::platform::agent_cli::aside_default_model)
}

fn status_with(manifest: &Path, runtime: RuntimeCall) -> anyhow::Result<IntegrationStatus> {
    let Some(record) = engine::read_record(&manifest.with_extension("aside.json"))? else {
        return Ok(IntegrationStatus {
            state: "unmanaged",
            mode: None,
            detail: super::DETAIL,
            error: None,
        });
    };
    if !engine::is_managed(manifest)? {
        return Ok(IntegrationStatus {
            state: "drift",
            mode: Some("native_runtime"),
            detail: super::DETAIL,
            error: Some("Aside account configuration was edited.".into()),
        });
    }
    let account = record["account"]
        .as_str()
        .context("invalid Aside account record")?;
    let actual = runtime(account, None);
    let managed = actual
        .as_ref()
        .is_ok_and(|actual| *actual == record["after"]);
    Ok(IntegrationStatus {
        state: if managed {
            "managed"
        } else if actual.is_ok() && record["phase"] == "managed" {
            "drift"
        } else {
            "pending"
        },
        mode: Some("native_runtime"),
        detail: if managed {
            "Aside confirmed the selected model in the captured account."
        } else {
            "Account files are staged; restart Aside or reconnect to confirm the runtime model."
        },
        error: (!managed).then(|| "Aside runtime has not confirmed the staged selection.".into()),
    })
}

pub(super) fn remove(manifest: &Path) -> anyhow::Result<()> {
    remove_with(manifest, crate::platform::agent_cli::aside_default_model)
}

fn remove_with(manifest: &Path, runtime: RuntimeCall) -> anyhow::Result<()> {
    let record_path = manifest.with_extension("aside.json");
    let Some(record) = engine::read_record(&record_path)? else {
        return engine::remove(manifest);
    };
    if manifest.exists() {
        anyhow::ensure!(
            engine::is_managed(manifest)?,
            "Aside account configuration was edited"
        );
    }
    let account = record["account"]
        .as_str()
        .context("invalid Aside account record")?;
    let actual = runtime(account, None);
    if let Ok(actual) = &actual {
        anyhow::ensure!(
            *actual == record["after"] || *actual == record["before"],
            "Aside runtime selection was edited; refusing to overwrite it"
        );
    }
    engine::remove(manifest)?;
    if actual.is_ok_and(|actual| actual == record["after"]) {
        anyhow::ensure!(
            runtime(account, Some(&record["before"]))? == record["before"],
            "Aside did not restore its prior model"
        );
    }
    engine::remove_record(&record_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline(_: &str, _: Option<&Value>) -> anyhow::Result<Value> {
        anyhow::bail!("offline")
    }

    #[test]
    fn offline_staging_tracks_the_captured_account_and_restores_exact_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("aside");
        let account = root.join("u/17");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(root.join("accounts.json"), r#"{"currentAccountId":"u17"}"#).unwrap();
        let original = r#"{"defaultModel":{"provider":"native","modelId":"old"},"theme":"dark"}"#;
        std::fs::write(account.join("settings.json"), original).unwrap();
        let manifest = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("manifest.json");
        let models = vec![FrameworkModel {
            id: "p/m".into(),
            name: "M".into(),
            context_window: 1000,
            max_tokens: 100,
            supports_image: false,
            protocol: crate::provider::ProviderProtocol::OpenAiChat,
            efforts: vec![],
        }];
        install_at(
            &manifest,
            &root,
            ("http://127.0.0.1:8080", "private-key"),
            &models,
            offline,
        )
        .unwrap();
        assert_eq!(status_with(&manifest, offline).unwrap().state, "pending");
        std::fs::write(root.join("accounts.json"), r#"{"currentAccountId":99}"#).unwrap();
        remove_with(&manifest, offline).unwrap();
        assert_eq!(
            std::fs::read_to_string(account.join("settings.json")).unwrap(),
            original
        );
        assert!(!account.join("models.json").exists());
        assert!(!root.join("u/99").exists());
    }
}
