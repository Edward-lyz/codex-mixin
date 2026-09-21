use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use fs2::FileExt;
use serde_json::Value;

use super::{MANAGED_HOOK_MARKER, REPORT_EVENTS};

use crate::cli::atomic_file::write_atomic_if_changed;
use codex_mixin::config::load_stored_config;

pub fn reporting_enabled() -> anyhow::Result<bool> {
    Ok(load_stored_config()?
        .map(|config| {
            config
                .providers
                .iter()
                .any(|provider| provider.enabled && provider.request_policy.baidu_code_report)
        })
        .unwrap_or(false))
}

pub fn sync_installation() -> anyhow::Result<()> {
    let enabled = reporting_enabled()?;
    let hooks_path = codex_mixin::platform::home_dir_required()?.join(".codex/hooks.json");
    sync_installation_at(&hooks_path, enabled)
}

pub fn sync_installation_at(hooks_path: &Path, enabled: bool) -> anyhow::Result<()> {
    if !enabled && !hooks_path.exists() {
        return Ok(());
    }
    if let Some(parent) = hooks_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = hooks_path.with_file_name("hooks.json.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("open Codex hooks lock {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("lock Codex hooks configuration {}", hooks_path.display()))?;
    let mut document = if hooks_path.exists() {
        let raw = fs::read(hooks_path)?;
        // Codex (or a partial/interrupted write) can leave an empty or
        // whitespace-only hooks.json. Treat that as a fresh document instead of
        // failing the whole add-provider / install-codex flow with
        // "expected value at line 1 column 1".
        if raw.iter().all(u8::is_ascii_whitespace) {
            serde_json::json!({ "hooks": {} })
        } else {
            serde_json::from_slice::<Value>(&raw).with_context(|| {
                format!("parse Codex hooks configuration {}", hooks_path.display())
            })?
        }
    } else {
        serde_json::json!({ "hooks": {} })
    };
    let hooks = document
        .as_object_mut()
        .context("Codex hooks configuration root is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("Codex hooks field is not an object")?;

    // Always strip our previously-installed managed commands first (idempotent).
    for event_name in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
    ] {
        if let Some(groups) = hooks.get_mut(event_name).and_then(Value::as_array_mut) {
            for group in groups.iter_mut() {
                if let Some(commands) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    commands.retain(|command| {
                        !command
                            .get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|value| value.contains(MANAGED_HOOK_MARKER))
                    });
                }
            }
            groups.retain(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|commands| !commands.is_empty())
            });
        }
        if hooks
            .get(event_name)
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            hooks.remove(event_name);
        }
    }

    if enabled {
        let executable = if cfg!(target_os = "macos") {
            let default_app =
                PathBuf::from("/Applications/Codex Mixin.app/Contents/Resources/codex-mixin");
            if default_app.is_file() {
                default_app
            } else {
                std::env::current_exe().context("resolve codex-mixin executable")?
            }
        } else {
            std::env::current_exe().context("resolve codex-mixin executable")?
        };
        let executable = shell_quote(&executable.to_string_lossy());
        for (event_name, event_argument) in REPORT_EVENTS {
            let group = serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": format!("{executable} report-hook --event {event_argument}"),
                    "timeout": 30,
                    "statusMessage": "Reporting Baidu AI code usage"
                }]
            });
            hooks
                .entry(event_name)
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .context("Codex hook event is not an array")?
                .push(group);
        }
    }

    let mut encoded = serde_json::to_vec_pretty(&document)?;
    encoded.push(b'\n');
    write_atomic_if_changed(hooks_path, &encoded)
        .with_context(|| format!("write Codex hooks configuration {}", hooks_path.display()))?;
    Ok(())
}

fn shell_quote(value: &str) -> String {
    if cfg!(windows) {
        // Codex on Windows resolves hook commands through the Windows shell,
        // which does not treat single quotes as a path delimiter. Use double
        // quotes and escape embedded quotes so paths with spaces still run.
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_hooks_file_is_treated_as_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks.json");
        // An empty file previously failed with "expected value at line 1".
        std::fs::write(&hooks, b"").unwrap();
        sync_installation_at(&hooks, false).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&hooks).unwrap()).unwrap();
        assert!(value.get("hooks").is_some());
    }

    #[test]
    fn whitespace_only_hooks_file_is_treated_as_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks.json");
        std::fs::write(&hooks, b"  \r\n\t").unwrap();
        sync_installation_at(&hooks, false).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&hooks).unwrap()).unwrap();
        assert!(value.get("hooks").is_some());
    }

    #[test]
    fn malformed_hooks_file_still_errors() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks.json");
        std::fs::write(&hooks, b"not json").unwrap();
        assert!(sync_installation_at(&hooks, false).is_err());
    }
}
