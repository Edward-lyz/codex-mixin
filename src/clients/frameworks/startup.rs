//! Reversible user-shell entries. Credentials remain in the existing launch journal.
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Value, json};

use super::engine;

pub(super) fn required(id: &str) -> bool {
    matches!(id, "agy" | "cursor-local" | "muse" | "commandcode")
        || (cfg!(feature = "client-auth") && matches!(id, "fx" | "empryo" | "ante"))
}

pub(super) fn is_installed(manifest: &Path) -> anyhow::Result<bool> {
    let Some(record) = engine::read_record(&manifest.with_extension("startup.json"))? else {
        return Ok(false);
    };
    for entry in entries(&record)? {
        let (path, block) = entry_fields(entry)?;
        let current = profile_text(&path)?.unwrap_or_default();
        anyhow::ensure!(
            current.matches(block).count() == 1,
            "managed startup entry changed; restore it before reconnecting or disconnecting"
        );
    }
    Ok(true)
}

pub(super) fn install(id: &str, manifest: &Path) -> anyhow::Result<()> {
    let executable = std::env::current_exe().context("locate Mixin executable for startup")?;
    let config = std::path::absolute(crate::config::stored_config_path())?;
    install_at(
        id,
        manifest,
        crate::platform::agent_startup::profiles(id, &executable, &config)?,
    )
}

pub(super) fn install_at(
    id: &str,
    manifest: &Path,
    profiles: Vec<(PathBuf, String)>,
) -> anyhow::Result<()> {
    let record_path = manifest.with_extension("startup.json");
    let record = if let Some(record) = engine::read_record(&record_path)? {
        anyhow::ensure!(
            record["id"] == id,
            "startup journal belongs to another agent"
        );
        record
    } else {
        let mut profile_entries = Vec::new();
        for (path, script) in profiles {
            let before = profile_text(&path)?;
            anyhow::ensure!(
                !before
                    .as_deref()
                    .unwrap_or_default()
                    .contains(&format!("# >>> codex-mixin managed {id} >>>")),
                "unowned startup entry exists; reconcile it before connecting"
            );
            // Include our separator in the owned bytes so exact originals roundtrip.
            let block = format!("\n{script}");
            let after = format!("{}{block}", before.as_deref().unwrap_or_default());
            profile_entries.push(json!({"path":path,"before":before,"after":after,"block":block}));
        }
        let record = json!({"version":1,"id":id,"entries":profile_entries});
        // Preflight every profile before the durable write-ahead record or mutations.
        entries(&record)?;
        engine::write_record(&record_path, &record)?;
        record
    };
    for entry in entries(&record)? {
        let (path, block) = entry_fields(entry)?;
        let current = profile_text(&path)?;
        if current
            .as_deref()
            .is_some_and(|text| text.matches(block).count() == 1)
        {
            continue;
        }
        anyhow::ensure!(
            json!(current) == entry["before"],
            "shell profile changed during startup install; refusing to overwrite it"
        );
        let after = entry["after"]
            .as_str()
            .context("invalid startup journal contents")?;
        save_profile(&path, Some(after))?;
    }
    anyhow::ensure!(
        is_installed(manifest)?,
        "startup entries were not confirmed"
    );
    Ok(())
}

pub(super) fn remove(manifest: &Path) -> anyhow::Result<()> {
    let record_path = manifest.with_extension("startup.json");
    let Some(record) = engine::read_record(&record_path)? else {
        return Ok(());
    };
    let mut changes = Vec::new();
    for entry in entries(&record)? {
        let (path, block) = entry_fields(entry)?;
        let current = profile_text(&path)?;
        if json!(current) == entry["before"] {
            continue;
        }
        let current = current
            .context("managed startup profile disappeared; reconcile it before disconnecting")?;
        anyhow::ensure!(
            current.matches(block).count() == 1,
            "managed startup entry was edited; refusing to overwrite it"
        );
        let restored = if current
            == entry["after"]
                .as_str()
                .context("invalid startup journal contents")?
        {
            entry["before"].as_str().map(str::to_owned)
        } else {
            let (prefix, suffix) = current
                .split_once(block)
                .context("startup entry disappeared")?;
            let separator = if !prefix.is_empty() && !prefix.ends_with('\n') && !suffix.is_empty() {
                "\n"
            } else {
                ""
            };
            Some(current.replacen(block, separator, 1))
        };
        changes.push((path, restored));
    }
    // Verify all blocks before removing any of them. Retry also accepts restored profiles.
    for (path, restored) in changes {
        save_profile(&path, restored.as_deref())?;
    }
    engine::remove_record(&record_path)
}

fn entries(record: &Value) -> anyhow::Result<&Vec<Value>> {
    let entries = record["entries"]
        .as_array()
        .context("invalid startup journal entries")?;
    anyhow::ensure!(
        record["version"] == 1 && !entries.is_empty() && entries.len() <= 8,
        "invalid startup journal"
    );
    Ok(entries)
}

fn entry_fields(entry: &Value) -> anyhow::Result<(PathBuf, &str)> {
    let path = PathBuf::from(
        entry["path"]
            .as_str()
            .context("invalid startup profile path")?,
    );
    engine::validate_path(&path)?;
    let block = entry["block"]
        .as_str()
        .filter(|block| !block.is_empty())
        .context("invalid startup entry")?;
    Ok((path, block))
}

fn profile_text(path: &Path) -> anyhow::Result<Option<String>> {
    engine::validate_path(path)?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read shell profile"),
    };
    let mut text = String::new();
    file.take(1_048_577)
        .read_to_string(&mut text)
        .context("shell profile must contain UTF-8 text")?;
    anyhow::ensure!(text.len() <= 1_048_576, "shell profile is too large");
    Ok(Some(text))
}

fn save_profile(path: &Path, text: Option<&str>) -> anyhow::Result<()> {
    engine::validate_path(path)?;
    if let Some(text) = text {
        let parent = path.parent().context("shell profile has no parent")?;
        if !parent.exists() {
            crate::platform::ensure_owner_only_dir(parent)?;
        }
        crate::platform::write_owner_only(path, text.as_bytes())
    } else {
        engine::remove_record(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_restore_exactly_and_preserve_unrelated_edits() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let profile = root.join("profile");
        let new_profile = root.join("new-profile");
        let manifest = root.join("agy.json");
        std::fs::write(&profile, "export ORIGINAL=1").unwrap();
        let profiles = vec![
            (
                profile.clone(),
                "# owned agy\nalias agy='mixin agent run agy --'\n".into(),
            ),
            (new_profile.clone(), "# owned new\n".into()),
        ];
        install_at("agy", &manifest, profiles.clone()).unwrap();
        install_at("agy", &manifest, profiles).unwrap();
        assert!(is_installed(&manifest).unwrap());
        let text = std::fs::read_to_string(&profile).unwrap();
        std::fs::write(&profile, format!("{text}export ADDED=2\n")).unwrap();
        remove(&manifest).unwrap();
        assert_eq!(
            std::fs::read_to_string(&profile).unwrap(),
            "export ORIGINAL=1\nexport ADDED=2\n"
        );
        assert!(!new_profile.exists());
        assert!(!manifest.with_extension("startup.json").exists());
        remove(&manifest).unwrap();
    }

    #[test]
    fn edited_entries_block_removal_without_changing_other_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let profile = root.join("profile");
        let second = root.join("second");
        let manifest = root.join("muse.json");
        install_at(
            "muse",
            &manifest,
            vec![
                (profile.clone(), "owned muse\n".into()),
                (second.clone(), "owned second\n".into()),
            ],
        )
        .unwrap();
        let original = std::fs::read(&second).unwrap();
        std::fs::write(&profile, "edited owned entry").unwrap();
        assert!(is_installed(&manifest).is_err());
        assert!(remove(&manifest).is_err());
        assert_eq!(std::fs::read(&second).unwrap(), original);
        assert!(manifest.with_extension("startup.json").exists());
    }
}
