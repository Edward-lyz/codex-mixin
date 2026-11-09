//! Recover the reporting model for a DUCX hook event.
//!
//! Only Codex puts the catalog slug in its hook payload. Claude Code omits
//! `model` on prompt/stop events and uses client-side names (`[1m]` context
//! suffixes and `claude-sonnet-mixin-<hex>` route ids), DSH hooks never carry
//! it, and Grok Build loads the Claude Code hooks unchanged, so the hook
//! resolves the model from local state before the upload path runs.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Value, json};

use crate::clients::{claude_desktop, dsh, grok};
use crate::config::load_stored_config_read_only;
use crate::platform::write_atomic_if_changed;
use crate::provider::{ProviderDefinition, ProviderModelSource, catalog_model_slug};
use crate::usage::latest_session_turn;

/// Per-session cache of the last model resolved for reporting.
const MODEL_CACHE_DIRECTORY: &str = "ducx-report-models";
const ROUTE_PREFIX: &str = "claude-sonnet-mixin-";
const CONTEXT_SUFFIX: &str = "[1m]";

/// Local state directories and the client homes the resolver may read.
pub struct ReportPaths<'a> {
    /// Directory writable by the hook (normal state dir or the sandboxed
    /// fallback used for queued reports and the model cache).
    pub report_state: &'a Path,
    /// Gateway state directory that owns `usage.sqlite3`.
    pub gateway_state: &'a Path,
    pub dsh_home: &'a Path,
    pub grok_home: &'a Path,
}

/// Resolve the catalog slug and its reporting provider for one hook event.
///
/// On success the resolved slug is written back into `body["model"]` so the
/// queued payload and the DUCX upload see the same value.
pub fn resolve_model(
    event: &str,
    body: &mut Value,
    paths: &ReportPaths<'_>,
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
        record_model(paths, &session_id, &resolved)?;
        set_model(body, &resolved.0);
        return Ok(Some(resolved));
    }

    if !session_id.is_empty() {
        // Grok keeps the active model in its session summary and has no
        // PostModelSwitch hook, so its summary is fresher than the cache.
        if let Some(model) = grok::session_model(paths.grok_home, &session_id)?
            && let Some(resolved) =
                resolved_from_local(event, paths, &session_id, "grok_session", &model, body)?
        {
            return Ok(Some(resolved));
        }
        if let Some(model) = cached_model(paths, &session_id)?
            && let Some(resolved) =
                resolved_from_local(event, paths, &session_id, "session_cache", &model, body)?
        {
            return Ok(Some(resolved));
        }
        if let Some(model) = usage_model(paths, &session_id)?
            && let Some(resolved) =
                resolved_from_local(event, paths, &session_id, "usage_ledger", &model, body)?
        {
            return Ok(Some(resolved));
        }
        if let Some(model) = dsh::session_model(paths.dsh_home, &session_id)?
            && let Some(resolved) =
                resolved_from_local(event, paths, &session_id, "dsh_session", &model, body)?
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
    paths: &ReportPaths<'_>,
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
    record_model(paths, session_id, &resolved)?;
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

fn record_model(
    paths: &ReportPaths<'_>,
    session_id: &str,
    resolved: &(String, ProviderDefinition),
) -> anyhow::Result<()> {
    if session_id.is_empty() {
        return Ok(());
    }
    let path = model_cache_path(paths, session_id)?;
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

fn cached_model(paths: &ReportPaths<'_>, session_id: &str) -> anyhow::Result<Option<String>> {
    let path = model_cache_path(paths, session_id)?;
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

fn model_cache_path(paths: &ReportPaths<'_>, session_id: &str) -> anyhow::Result<PathBuf> {
    validate_session_id(session_id)?;
    Ok(paths
        .report_state
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
fn usage_model(paths: &ReportPaths<'_>, session_id: &str) -> anyhow::Result<Option<String>> {
    let turn = match latest_session_turn(paths.gateway_state, session_id) {
        Ok(Some(turn)) => turn,
        Ok(None) => return Ok(None),
        Err(error) => {
            tracing::debug!(%error, "usage ledger is unavailable");
            return Ok(None);
        }
    };
    let Some(provider) = reporting_provider_by_id(&turn.provider_id)? else {
        return Ok(None);
    };
    if !provider_owns_model(&provider, &turn.catalog_slug) {
        return Ok(None);
    }
    Ok(Some(turn.catalog_slug))
}

pub fn validate_session_id(session_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !session_id.is_empty(),
        "DUCX report hook is missing session_id"
    );
    anyhow::ensure!(
        session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "DUCX report hook session_id contains unsupported characters"
    );
    Ok(())
}

fn reporting_provider(model: &str) -> anyhow::Result<Option<ProviderDefinition>> {
    Ok(reporting_providers()?
        .into_iter()
        .find(|provider| provider_owns_model(provider, model)))
}

pub fn reporting_provider_by_id(provider_id: &str) -> anyhow::Result<Option<ProviderDefinition>> {
    Ok(reporting_providers()?
        .into_iter()
        .find(|provider| provider.id == provider_id))
}

pub fn reporting_providers() -> anyhow::Result<Vec<ProviderDefinition>> {
    let Some(config) = load_stored_config_read_only()? else {
        return Ok(Vec::new());
    };
    Ok(config
        .providers
        .into_iter()
        .filter(|provider| {
            provider.enabled
                && provider.request_policy.baidu_code_report
                && provider.model_source == ProviderModelSource::BaiduOneApi
        })
        .collect())
}

/// True when `model` (as Codex sees it) maps to one of the provider's models,
/// matching the provider-qualified catalog slug for a selected model.
pub fn provider_owns_model(provider: &ProviderDefinition, model: &str) -> bool {
    provider
        .selected_models
        .iter()
        .any(|candidate| catalog_model_slug(candidate, &provider.id) == model)
}
