//! Recover the reporting model for a DUCX hook event.
//!
//! Only Codex puts the catalog slug in its hook payload. Claude Code omits
//! `model` on prompt/stop events and uses client-side names (`[1m]` context
//! suffixes and `claude-sonnet-mixin-<hex>` route ids), and DSH hooks never
//! carry it, so the hook resolves the model from local state before the
//! upload path runs.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Value, json};

use codex_mixin::clients::claude_desktop;
use codex_mixin::platform::write_atomic_if_changed;
use codex_mixin::provider::{ProviderDefinition, catalog_model_slug};

use super::transport::{
    provider_owns_model, reporting_provider, reporting_provider_by_id, reporting_providers,
};
use crate::cli::runtime::{report_state_dir, state_dir};

/// Per-session cache of the last model resolved for reporting.
const MODEL_CACHE_DIRECTORY: &str = "ducx-report-models";
/// DSH writes one session log per conversation; the model is recorded near the
/// start, so a bounded prefix keeps the hook cheap on long sessions.
const DSH_SESSION_HEAD_BYTES: u64 = 256 * 1024;
const ROUTE_PREFIX: &str = "claude-sonnet-mixin-";
const CONTEXT_SUFFIX: &str = "[1m]";
const DSH_MODEL_POINTERS: [&str; 5] = [
    "/data/header/config/model",
    "/data/model",
    "/data/route/model",
    "/data/source/model",
    "/data/message/source/model",
];

/// Resolve the catalog slug and its reporting provider for one hook event.
///
/// On success the resolved slug is written back into `body["model"]` so the
/// queued payload and the DUCX upload see the same value.
pub(super) fn resolve(
    event: &str,
    body: &mut Value,
) -> anyhow::Result<Option<(String, ProviderDefinition)>> {
    let session_id = body
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let payload_model = payload_model(event, body);

    if let Some(raw) = payload_model.as_deref()
        && let Some(resolved) = provider_for_model(raw)?
    {
        record_model(&session_id, &resolved)?;
        set_model(body, &resolved.0);
        return Ok(Some(resolved));
    }

    if !session_id.is_empty() {
        if let Some(model) = cached_model(&session_id)?
            && let Some(resolved) =
                resolved_from_local(event, &session_id, "session_cache", &model, body)?
        {
            return Ok(Some(resolved));
        }
        if let Some(model) = usage_model(&session_id)?
            && let Some(resolved) =
                resolved_from_local(event, &session_id, "usage_ledger", &model, body)?
        {
            return Ok(Some(resolved));
        }
        if let Some(model) = dsh_model(&session_id)?
            && let Some(resolved) =
                resolved_from_local(event, &session_id, "dsh_session", &model, body)?
        {
            return Ok(Some(resolved));
        }
    }

    let reason = if payload_model.is_some() {
        "no_matching_reporting_provider"
    } else {
        "missing_model"
    };
    tracing::info!(
        event,
        session_id = %session_id,
        reason,
        "DUCX reporting skipped"
    );
    Ok(None)
}

fn resolved_from_local(
    event: &str,
    session_id: &str,
    source: &str,
    model: &str,
    body: &mut Value,
) -> anyhow::Result<Option<(String, ProviderDefinition)>> {
    let Some(resolved) = provider_for_model(model)? else {
        return Ok(None);
    };
    tracing::info!(
        event,
        session_id = %session_id,
        source,
        model = %resolved.0,
        "DUCX reporting model resolved"
    );
    record_model(session_id, &resolved)?;
    set_model(body, &resolved.0);
    Ok(Some(resolved))
}

fn payload_model(event: &str, body: &Value) -> Option<String> {
    let fields: &[&str] = if event == "model-switch" {
        &["to_model", "model"]
    } else {
        &["model"]
    };
    fields.iter().find_map(|field| {
        body.get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_owned)
    })
}

fn set_model(body: &mut Value, model: &str) {
    if body.get("model").and_then(Value::as_str) == Some(model) {
        return;
    }
    if let Some(object) = body.as_object_mut() {
        object.insert("model".to_owned(), Value::String(model.to_owned()));
    }
}

fn model_candidates(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let mut candidates = vec![raw.to_owned()];
    if let Some(stripped) = strip_context_suffix(raw)
        && !candidates.contains(&stripped)
    {
        candidates.push(stripped);
    }
    if raw.starts_with(ROUTE_PREFIX)
        && let Ok(slug) = claude_desktop::route_model(raw)
        && !candidates.contains(&slug)
    {
        candidates.push(slug);
    }
    candidates
}

fn strip_context_suffix(model: &str) -> Option<String> {
    let suffix = CONTEXT_SUFFIX.len();
    if model.len() > suffix && model[model.len() - suffix..].eq_ignore_ascii_case(CONTEXT_SUFFIX) {
        Some(model[..model.len() - suffix].trim_end().to_owned())
    } else {
        None
    }
}

/// Match a client-side model name against the models of reporting providers.
fn provider_for_model(raw: &str) -> anyhow::Result<Option<(String, ProviderDefinition)>> {
    for candidate in model_candidates(raw) {
        if let Some(provider) = reporting_provider(&candidate)? {
            return Ok(Some((candidate, provider)));
        }
        // Claude Code and DSH can report a bare upstream name such as
        // `deepseek-v4.1-flash`; accept a case-insensitive match on either the
        // selected model or its catalog slug so a client-side suffix does not
        // drop the event.
        for provider in reporting_providers()? {
            for selected in &provider.selected_models {
                let slug = catalog_model_slug(selected, &provider.id);
                if selected.eq_ignore_ascii_case(&candidate)
                    || slug.eq_ignore_ascii_case(&candidate)
                {
                    return Ok(Some((slug, provider)));
                }
            }
        }
    }
    Ok(None)
}

fn record_model(session_id: &str, resolved: &(String, ProviderDefinition)) -> anyhow::Result<()> {
    if session_id.is_empty() {
        return Ok(());
    }
    let path = model_cache_path(session_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create DUCX model cache {}", parent.display()))?;
    }
    let payload = serde_json::to_vec(&json!({
        "provider_id": resolved.1.id,
        "model": resolved.0,
    }))?;
    write_atomic_if_changed(&path, &payload)
        .with_context(|| format!("record DUCX reporting model {}", path.display()))?;
    Ok(())
}

fn cached_model(session_id: &str) -> anyhow::Result<Option<String>> {
    let path = model_cache_path(session_id)?;
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read DUCX reporting model {}", path.display()));
        }
    };
    let Ok(value) = serde_json::from_slice::<Value>(&raw) else {
        return Ok(None);
    };
    let model = value.get("model").and_then(Value::as_str);
    let provider_id = value.get("provider_id").and_then(Value::as_str);
    let (Some(model), Some(provider_id)) = (model, provider_id) else {
        return Ok(None);
    };
    let Some(provider) = reporting_provider_by_id(provider_id)? else {
        return Ok(None);
    };
    if !provider_owns_model(&provider, model) {
        return Ok(None);
    }
    Ok(Some(model.to_owned()))
}

fn model_cache_path(session_id: &str) -> anyhow::Result<PathBuf> {
    super::validate_session_id(session_id)?;
    Ok(report_state_dir()
        .join(MODEL_CACHE_DIRECTORY)
        .join(format!("{session_id}.json")))
}

/// Look up the last turn the gateway recorded for this client session.
///
/// Claude Code and Codex use the same session id for the hook and for the
/// gateway routing key, so the usage ledger is the most reliable source once a
/// turn has completed. Sandboxed hooks may be denied read-only WAL access to
/// the ledger, in which case this source is skipped instead of failing the
/// whole hook.
fn usage_model(session_id: &str) -> anyhow::Result<Option<String>> {
    let path = state_dir().join("usage.sqlite3");
    if !path.is_file() {
        return Ok(None);
    }
    let connection = match rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(connection) => connection,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "usage ledger is unavailable");
            return Ok(None);
        }
    };
    let has_table: bool = match connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'token_usage_turns')",
        [],
        |row| row.get(0),
    ) {
        Ok(has_table) => has_table,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "usage ledger is unavailable");
            return Ok(None);
        }
    };
    if !has_table {
        return Ok(None);
    }
    let tag = codex_mixin::usage_session_tag(session_id);
    let row = connection.query_row(
        "SELECT provider_id, catalog_slug FROM token_usage_turns \
         WHERE session = ?1 ORDER BY at_ms DESC LIMIT 1",
        [&tag],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    );
    let (provider_id, model) = match row {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "usage ledger lookup failed");
            return Ok(None);
        }
    };
    let Some(provider) = reporting_provider_by_id(&provider_id)? else {
        return Ok(None);
    };
    if !provider_owns_model(&provider, &model) {
        return Ok(None);
    }
    Ok(Some(model))
}

/// DSH hooks run through a bridge whose payload has no model and no usable
/// transcript path, so read the session log the app keeps under `$DSH_HOME`.
fn dsh_model(session_id: &str) -> anyhow::Result<Option<String>> {
    let home = crate::cli::dsh::default_dsh_home();
    if let Some(model) = dsh_session_log_model(&home, session_id)? {
        return Ok(Some(model));
    }
    dsh_projcache_model(&home, session_id)
}

fn dsh_session_log_model(home: &Path, session_id: &str) -> anyhow::Result<Option<String>> {
    let sessions = home.join("sessions");
    let entries = match std::fs::read_dir(&sessions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("read DSH sessions {}", sessions.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        // DSH may hand the bridge either a bare session UUID or the
        // `session-<uuid>` header id, so accept both directory names.
        for directory in [format!("session-{session_id}"), session_id.to_owned()] {
            let path = entry.path().join(directory).join("session.v4.jsonl.zstd");
            if path.is_file() {
                return dsh_zstd_model(&path);
            }
        }
    }
    Ok(None)
}

fn dsh_zstd_model(path: &Path) -> anyhow::Result<Option<String>> {
    let file = File::open(path).with_context(|| format!("open DSH session {}", path.display()))?;
    let decoder = zstd::stream::read::Decoder::new(file)
        .with_context(|| format!("decode DSH session {}", path.display()))?;
    let mut head = Vec::new();
    decoder
        .take(DSH_SESSION_HEAD_BYTES)
        .read_to_end(&mut head)
        .with_context(|| format!("read DSH session {}", path.display()))?;
    for line in head.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        for pointer in DSH_MODEL_POINTERS {
            if let Some(model) = value
                .pointer(pointer)
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty())
            {
                return Ok(Some(model.to_owned()));
            }
        }
    }
    Ok(None)
}

fn dsh_projcache_model(home: &Path, session_id: &str) -> anyhow::Result<Option<String>> {
    let directory = home.join("storages/session_projcache/sessions");
    for name in [
        format!("session-{session_id}.json"),
        format!("{session_id}.json"),
    ] {
        let path = directory.join(name);
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read DSH session cache {}", path.display()));
            }
        };
        let Ok(value) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };
        if let Some(model) = value
            .pointer("/record/rows/modelSelection/val/lastUsed/model")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
        {
            return Ok(Some(model.to_owned()));
        }
    }
    Ok(None)
}
