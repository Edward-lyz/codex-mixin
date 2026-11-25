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
    let id = record["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .context("invalid startup journal agent id")?;
    let mut changes = Vec::new();
    for entry in entries(&record)? {
        let (path, block) = entry_fields(entry)?;
        let current = profile_text(&path)?;
        if json!(current) == entry["before"] {
            continue;
        }
        let current = current
            .context("managed startup profile disappeared; reconcile it before disconnecting")?;
        let range = removable_block_range(&current, block, id)?;
        let restored = if current
            == entry["after"]
                .as_str()
                .context("invalid startup journal contents")?
        {
            entry["before"].as_str().map(str::to_owned)
        } else {
            let prefix = &current[..range.start];
            let suffix = &current[range.end..];
            let separator = if !prefix.is_empty() && !prefix.ends_with('\n') && !suffix.is_empty() {
                "\n"
            } else {
                ""
            };
            let mut next = String::with_capacity(prefix.len() + separator.len() + suffix.len());
            next.push_str(prefix);
            next.push_str(separator);
            next.push_str(suffix);
            if next.is_empty() && entry["before"].is_null() {
                None
            } else {
                Some(next)
            }
        };
        changes.push((path, restored));
    }
    // Verify all blocks before removing any of them. Retry also accepts restored profiles.
    for (path, restored) in changes {
        save_profile(&path, restored.as_deref())?;
    }
    engine::remove_record(&record_path)
}

fn removable_block_range(
    current: &str,
    block: &str,
    id: &str,
) -> anyhow::Result<std::ops::Range<usize>> {
    if current.matches(block).count() == 1 {
        let start = current.find(block).context("startup entry disappeared")?;
        return Ok(start..start + block.len());
    }
    if let Some(range) = empty_fence_range(current, id) {
        return Ok(range);
    }
    anyhow::bail!("managed startup entry was edited; refusing to overwrite it");
}

/// External installers (for example the agy CLI installer stripping `alias agy=...`
/// lines) can leave an empty managed fence behind. Match only a single exact pair of
/// adjacent marker lines, refusing any inline text or non-empty edits.
fn empty_fence_range(current: &str, id: &str) -> Option<std::ops::Range<usize>> {
    let start = format!("# >>> codex-mixin managed {id} >>>");
    let end = format!("# <<< codex-mixin managed {id} <<<");
    if current.matches(&start).count() != 1 || current.matches(&end).count() != 1 {
        return None;
    }
    let mut offset = 0;
    let mut lines = Vec::new();
    for raw in current.split_inclusive('\n') {
        let text = raw.strip_suffix('\n').unwrap_or(raw);
        let text = text.strip_suffix('\r').unwrap_or(text);
        lines.push((offset, offset + raw.len(), text));
        offset += raw.len();
    }
    let index = lines.iter().position(|(_, _, text)| *text == start)?;
    let (_, end_offset, next_text) = lines.get(index + 1)?;
    if *next_text != end {
        return None;
    }
    let mut start_offset = lines[index].0;
    // Include the separator newline `install_at` prepended before the fence:
    // a blank line when the prior content ended with a newline, or the single
    // separating newline immediately before the fence otherwise.
    if index > 0 && lines[index - 1].2.is_empty() {
        start_offset = lines[index - 1].0;
    } else if start_offset > 0 && current.as_bytes()[start_offset - 1] == b'\n' {
        start_offset = if start_offset >= 2 && current.as_bytes()[start_offset - 2] == b'\r' {
            start_offset - 2
        } else {
            start_offset - 1
        };
    }
    Some(start_offset..*end_offset)
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
