use std::fs;
use std::path::Path;

use anyhow::Context;
use fs2::FileExt;
use serde_json::Value;

use super::REPORT_EVENTS;

use codex_mixin::config::load_stored_config;
use codex_mixin::platform::write_atomic_if_changed;

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
                            .is_some_and(codex_mixin::platform::is_report_hook_command)
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
        let executable = codex_mixin::platform::installation::installed_cli_executable()?;
        for (event_name, event_argument) in REPORT_EVENTS {
            let group = serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": codex_mixin::platform::report_hook_command(&executable, event_argument),
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
