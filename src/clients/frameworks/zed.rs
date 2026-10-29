use std::path::Path;

use anyhow::Context;
use serde_json::json;

use super::{FrameworkModel, PROVIDER, engine, field};
use crate::clients::IntegrationStatus;
use crate::platform::agent_credentials::{zed_read, zed_write};

const DETAIL: &str = "Zed settings and its OS credential are owned together; disconnect restores both. Restart Zed after changing settings.";

pub(super) fn status(manifest: &Path) -> anyhow::Result<IntegrationStatus> {
    status_with(manifest, zed_read)
}

fn status_with(
    manifest: &Path,
    read: impl Fn(&str) -> anyhow::Result<Option<String>>,
) -> anyhow::Result<IntegrationStatus> {
    let Some(record) = engine::read_record(&manifest.with_extension("credential.json"))? else {
        return Ok(IntegrationStatus {
            state: "unmanaged",
            mode: None,
            detail: DETAIL,
            error: None,
        });
    };
    let server = record["server"]
        .as_str()
        .context("invalid Zed credential record")?;
    let key = record["after"]
        .as_str()
        .context("invalid Zed credential record")?;
    let managed = engine::is_managed(manifest)? && read(server)?.as_deref() == Some(key);
    Ok(IntegrationStatus {
        state: if managed { "managed" } else if record["phase"] == "preparing" { "pending" } else { "drift" },
        mode: Some("native_credentials"),
        detail: DETAIL,
        error: (!managed).then(|| "Zed credential or settings require reconciliation; reconnect to finish an interrupted install.".to_owned()),
    })
}

pub(super) fn install(
    manifest: &Path,
    base: &str,
    key: &str,
    models: &[FrameworkModel],
) -> anyhow::Result<()> {
    install_at(
        manifest,
        &crate::platform::agent_cli::zed_settings_path()?,
        (base, key),
        models,
        zed_read,
        zed_write,
    )
}

fn install_at(
    manifest: &Path,
    settings: &Path,
    binding: (&str, &str),
    models: &[FrameworkModel],
    read: impl Fn(&str) -> anyhow::Result<Option<String>>,
    write: impl Fn(&str, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let first = models
        .first()
        .context("no selected models are available for Zed")?;
    let (base, key) = binding;
    let server = format!("{base}/v1");
    let record_path = manifest.with_extension("credential.json");
    let mut record = match engine::read_record(&record_path)? {
        Some(record) => {
            anyhow::ensure!(
                record["server"] == server && record["after"] == key,
                "Zed binding changed; disconnect the previous integration first"
            );
            if record["phase"] == "managed" {
                anyhow::ensure!(
                    status_with(manifest, &read)?.state == "managed",
                    "Zed integration drifted"
                );
                return Ok(());
            }
            record
        }
        None => json!({"server":server,"before":read(&server)?,"after":key,"phase":"preparing"}),
    };
    engine::write_record(&record_path, &record)?;
    let entries: Vec<_> = models
        .iter()
        .map(|model| {
            let context = if model.context_window == 0 {
                128_000
            } else {
                model.context_window
            };
            let output = model.max_tokens.min(context);
            let mut entry = json!({"name":model.id,"display_name":model.name,
            "max_tokens":context.saturating_add(output),"max_output_tokens":output,
            "capabilities":{"tools":true,"images":model.supports_image,"parallel_tool_calls":false,
                "prompt_cache_key":false,"chat_completions":true}});
            if !model.efforts.is_empty() {
                entry["reasoning_effort"] = json!(
                    ["high", "medium", "low", "minimal"]
                        .into_iter()
                        .find(|level| model.efforts.iter().any(|effort| effort == level))
                        .unwrap_or("high")
                );
            }
            entry
        })
        .collect();
    engine::apply(
        manifest,
        vec![engine::Patch {
            path: settings.to_path_buf(),
            format: engine::Format::Json,
            fields: vec![
                (
                    vec![
                        "language_models".into(),
                        "openai_compatible".into(),
                        PROVIDER.into(),
                    ],
                    json!({"api_url":server,"available_models":entries}),
                ),
                field(
                    &["agent", "default_model"],
                    json!({"provider":PROVIDER,"model":first.id}),
                ),
            ],
        }],
    )?;
    let current = read(&server)?;
    anyhow::ensure!(
        current.as_deref() == record["before"].as_str() || current.as_deref() == Some(key),
        "Zed credential changed during install; refusing to overwrite it"
    );
    write(&server, Some(key))?;
    anyhow::ensure!(
        read(&server)?.as_deref() == Some(key),
        "Zed credential write was not confirmed"
    );
    record["phase"] = json!("managed");
    engine::write_record(&record_path, &record)
}

pub(super) fn remove(manifest: &Path) -> anyhow::Result<()> {
    remove_with(manifest, zed_read, zed_write)
}

fn remove_with(
    manifest: &Path,
    read: impl Fn(&str) -> anyhow::Result<Option<String>>,
    write: impl Fn(&str, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let record_path = manifest.with_extension("credential.json");
    let Some(record) = engine::read_record(&record_path)? else {
        return engine::remove(manifest);
    };
    let server = record["server"]
        .as_str()
        .context("invalid Zed credential record")?;
    let current = read(server)?;
    let before = record["before"].as_str();
    let after = record["after"]
        .as_str()
        .context("invalid Zed credential record")?;
    anyhow::ensure!(
        current.as_deref() == Some(after) || current.as_deref() == before,
        "Zed credential was edited; refusing to overwrite it"
    );
    engine::remove(manifest)?;
    if current.as_deref() != before {
        write(server, before)?;
    }
    anyhow::ensure!(
        read(server)?.as_deref() == before,
        "Zed credential restore was not confirmed"
    );
    engine::remove_record(&record_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn settings_and_credentials_restore_without_overwriting_user_edits() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().canonicalize().unwrap().join("zed.json");
        let settings = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("settings.json");
        let original = b"{\"agent\":{\"default_model\":{\"provider\":\"native\",\"model\":\"old\"}},\"theme\":\"custom\"}";
        std::fs::write(&settings, original).unwrap();
        let credential = RefCell::new(Some("original-key".to_owned()));
        let read = |_: &str| Ok(credential.borrow().clone());
        let write = |_: &str, value: Option<&str>| {
            *credential.borrow_mut() = value.map(str::to_owned);
            Ok(())
        };
        let models = vec![FrameworkModel {
            id: "provider/model".into(),
            name: "Model".into(),
            context_window: 64_000,
            max_tokens: 4_096,
            supports_image: true,
            protocol: crate::provider::ProviderProtocol::OpenAiChat,
            efforts: vec!["high".into()],
        }];
        install_at(
            &manifest,
            &settings,
            ("http://127.0.0.1:8080", "managed-key"),
            &models,
            read,
            write,
        )
        .unwrap();
        assert_eq!(status_with(&manifest, read).unwrap().state, "managed");
        install_at(
            &manifest,
            &settings,
            ("http://127.0.0.1:8080", "managed-key"),
            &models,
            read,
            write,
        )
        .unwrap();
        *credential.borrow_mut() = Some("user-edited".into());
        assert!(remove_with(&manifest, read, write).is_err());
        assert_eq!(credential.borrow().as_deref(), Some("user-edited"));
        *credential.borrow_mut() = Some("managed-key".into());
        remove_with(&manifest, read, write).unwrap();
        assert_eq!(std::fs::read(&settings).unwrap(), original);
        assert_eq!(credential.borrow().as_deref(), Some("original-key"));
        assert!(!manifest.exists());
    }
}
