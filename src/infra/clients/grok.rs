//! Grok Build session state used to recover the reporting model.
//!
//! Grok discovers Claude Code hooks, whose payload has no model. The app
//! records the session's active model in
//! `$GROK_HOME/sessions/<encoded-cwd>/<session-id>/summary.json`.

use std::path::{Component, Path};

use anyhow::{Context, ensure};
use serde_json::Value;

/// Read `current_model_id` from the summary of `session_id`.
///
/// `session_id` becomes one path component, so reject separators and parent
/// components before joining it into the sessions tree.
pub fn session_model(grok_home: &Path, session_id: &str) -> anyhow::Result<Option<String>> {
    let mut components = Path::new(session_id).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_)))
            && components.next().is_none()
            && !session_id.contains(['/', '\\']),
        "Grok session ID contains unsupported characters"
    );
    let sessions = grok_home.join("sessions");
    let entries = match std::fs::read_dir(&sessions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read Grok sessions {}", sessions.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("read Grok sessions {}", sessions.display()))?;
        if !entry
            .file_type()
            .with_context(|| format!("inspect Grok sessions {}", sessions.display()))?
            .is_dir()
        {
            continue;
        }
        let session_directory = entry.path().join(session_id);
        if !session_directory.is_dir() {
            continue;
        }
        let path = session_directory.join("summary.json");
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("read Grok session {}", path.display()));
            }
        };
        // A malformed summary belongs to an unrelated or half-written session;
        // the next hook event retries instead of failing this one.
        let Ok(summary) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };
        if summary.pointer("/info/id").and_then(Value::as_str) != Some(session_id) {
            continue;
        }
        if let Some(model) = summary
            .get("current_model_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            return Ok(Some(model.to_owned()));
        }
    }
    Ok(None)
}
