//! Optional read-only names. A missing client store must not hide usage.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::gateway_access::GatewayClient;

pub(crate) const TITLE_CLIENTS: [GatewayClient; 3] = [
    GatewayClient::Codex,
    GatewayClient::Dsh,
    GatewayClient::Claude,
];
const METADATA_BYTES: u64 = 16 * 1024 * 1024;
const TRANSCRIPT_EDGE_BYTES: u64 = 128 * 1024;
const PREVIEW_CHARS: usize = 120;

pub(crate) fn visit_titles(
    client: GatewayClient,
    wanted: impl Fn(&str) -> bool,
    mut visit: impl FnMut(String, String),
) {
    if client == GatewayClient::Codex {
        super::codex::titles::visit_titles(|id, title| {
            if wanted(&id) {
                visit(id, title);
            }
        });
        return;
    }
    let mut titles = HashMap::new();
    let read = match client {
        GatewayClient::Dsh => client_home("DSH_HOME", ".dsh")
            .and_then(|home| read_dsh_titles(&home, &wanted, &mut titles)),
        GatewayClient::Claude => client_home("CLAUDE_CONFIG_DIR", ".claude")
            .and_then(|home| read_claude_titles(&home, &wanted, &mut titles)),
        _ => return,
    };
    if let Err(error) = read {
        tracing::warn!(client = client.display_name(), error = %error, "Client titles unavailable; using session previews or identifiers");
    }
    for (id, title) in titles {
        visit(id, title);
    }
}

fn client_home(variable: &str, directory: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(variable).filter(|path| !path.is_empty()) {
        return Ok(path.into());
    }
    Ok(crate::platform::home_dir_required()?.join(directory))
}

fn read_json(path: &Path) -> Result<Value> {
    let file = File::open(path).with_context(|| format!("open metadata {}", path.display()))?;
    anyhow::ensure!(
        file.metadata()?.len() <= METADATA_BYTES,
        "client metadata too large: {}",
        path.display()
    );
    serde_json::from_reader(file.take(METADATA_BYTES))
        .with_context(|| format!("parse metadata {}", path.display()))
}

fn read_dsh_titles(
    home: &Path,
    wanted: &impl Fn(&str) -> bool,
    titles: &mut HashMap<String, String>,
) -> Result<()> {
    let legacy = home.join("storages/session_projcache.json");
    if legacy.exists() {
        let metadata = read_json(&legacy)?;
        if let Some(sessions) = metadata
            .pointer("/tables/sessions")
            .and_then(Value::as_object)
        {
            for (id, record) in sessions {
                if wanted(id)
                    && let Some(title) = record.pointer("/rows/title/val").and_then(Value::as_str)
                    && !title.trim().is_empty()
                {
                    titles.insert(id.clone(), title.trim().to_owned());
                }
            }
        }
    }
    let directory = home.join("storages/session_projcache/sessions");
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&directory)? {
        let path = entry?.path();
        let Some(id) = path.file_stem().and_then(|id| id.to_str()) else {
            continue;
        };
        if path.extension().is_none_or(|extension| extension != "json") || !wanted(id) {
            continue;
        }
        let record = read_json(&path)?;
        // The per-session projection supersedes the older combined cache.
        titles.remove(id);
        if let Some(title) = record
            .pointer("/record/rows/title/val")
            .and_then(Value::as_str)
            && !title.trim().is_empty()
        {
            titles.insert(id.to_owned(), title.trim().to_owned());
        }
    }
    Ok(())
}

fn read_claude_titles(
    home: &Path,
    wanted: &impl Fn(&str) -> bool,
    titles: &mut HashMap<String, String>,
) -> Result<()> {
    let projects = home.join("projects");
    if !projects.exists() {
        return Ok(());
    }
    for project in std::fs::read_dir(projects)? {
        let project = project?;
        if !project.file_type()?.is_dir() {
            continue;
        }
        let index = project.path().join("sessions-index.json");
        if index.exists() {
            let index = read_json(&index)?;
            if let Some(entries) = index.get("entries").and_then(Value::as_array) {
                for entry in entries {
                    let Some(id) = entry.get("sessionId").and_then(Value::as_str) else {
                        continue;
                    };
                    if !wanted(id) {
                        continue;
                    }
                    if let Some(title) = ["customTitle", "summary", "firstPrompt"]
                        .into_iter()
                        .filter_map(|key| entry.get(key).and_then(Value::as_str))
                        .find(|title| !title.trim().is_empty())
                    {
                        titles.insert(id.to_owned(), title.trim().to_owned());
                    }
                }
            }
        }
        for entry in std::fs::read_dir(project.path())? {
            let path = entry?.path();
            let Some(id) = path.file_stem().and_then(|id| id.to_str()) else {
                continue;
            };
            if path
                .extension()
                .is_none_or(|extension| extension != "jsonl")
                || !wanted(id)
            {
                continue;
            }
            if let Some(title) = read_claude_title(&path, titles.get(id).cloned())? {
                titles.insert(id.to_owned(), title);
            }
        }
    }
    Ok(())
}

fn read_claude_title(path: &Path, indexed: Option<String>) -> Result<Option<String>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut head = Vec::new();
    (&mut file)
        .take(TRANSCRIPT_EDGE_BYTES)
        .read_to_end(&mut head)?;
    let mut preview = None;
    let mut renamed = None;
    let mut generated = None;
    // Bound I/O for large transcripts. Renames are appended; a prompt preview
    // comes from the head. Middle-only historic metadata needs a client index.
    let mut windows = vec![head];
    let start = size.saturating_sub(TRANSCRIPT_EDGE_BYTES);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
        let mut tail = Vec::new();
        file.take(TRANSCRIPT_EDGE_BYTES).read_to_end(&mut tail)?;
        if let Some(newline) = tail.iter().position(|byte| *byte == b'\n') {
            windows.push(tail[newline + 1..].to_vec());
        }
    }
    for window in windows {
        for line in window.split(|byte| *byte == b'\n') {
            let Ok(entry) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if entry.get("type").and_then(Value::as_str) == Some("custom-title") {
                renamed = entry
                    .get("customTitle")
                    .and_then(Value::as_str)
                    .filter(|title| !title.trim().is_empty())
                    .map(|title| title.trim().to_owned());
            }
            if entry.get("type").and_then(Value::as_str) == Some("ai-title") {
                generated = entry
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .filter(|title| !title.trim().is_empty())
                    .map(|title| title.trim().to_owned());
            }
            if preview.is_none() && indexed.is_none() {
                preview = entry
                    .get("message")
                    .and_then(request_preview)
                    .map(|preview| format!("Claude / {preview}"));
            }
        }
    }
    Ok(renamed.or(generated).or(indexed).or(preview))
}

/// A preview is explicitly labelled by the caller; it is not a generated title.
pub(crate) fn request_preview(message: &Value) -> Option<String> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let content = message.get("content")?;
    if let Some(text) = content.as_str() {
        return preview_text(text);
    }
    content
        .as_array()?
        .iter()
        .filter_map(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("text" | "input_text")
            )
            .then(|| part.get("text").and_then(Value::as_str))
            .flatten()
        })
        .find_map(preview_text)
}

fn preview_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty()
        || [
            "# AGENTS.md",
            "<environment_context",
            "<context_summary",
            "<turn_aborted",
            "<permissions instructions",
            "<user_instructions",
            "<external_codex_apps_open_page",
            "<system-reminder>",
            "[Request interrupted by user",
            "You are a helpful assistant.",
            "# Files mentioned by the user:",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return None;
    }
    let preview: String = text
        .split_whitespace()
        .flat_map(|word| word.chars().chain(std::iter::once(' ')))
        .take(PREVIEW_CHARS)
        .collect();
    Some(preview.trim_end().to_owned())
}
