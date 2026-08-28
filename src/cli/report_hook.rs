//! Mixin-level Baidu code-usage reporting.
//!
//! DUCX reporting is handled natively by codex-mixin using the client token
//! captured during DUCX warmup.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use anyhow::{Context, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::runtime::state_dir;
use codex_mixin::provider::ProviderDefinition;

mod installation;
mod queue;
mod transport;

pub(super) use installation::{reporting_enabled, sync_installation_at};
use transport::{
    post_json, post_raw_json, post_transcript, report_with_provider, reporting_provider,
    reporting_provider_by_id, reporting_providers,
};

const MANAGED_HOOK_MARKER: &str = " report-hook --event ";
/// (Codex hooks.json event name, our `--event` argument value).
const REPORT_EVENTS: [(&str, &str); 5] = [
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "user-prompt-submit"),
    ("PreToolUse", "pre-tool-use"),
    ("PostToolUse", "post-tool-use"),
    ("Stop", "stop"),
];

const REPORT_APPLY_PATCH_TOOL: &str = "apply_patch";

pub(super) fn sync_installation() -> anyhow::Result<()> {
    installation::sync_installation()?;
    crate::cli::opencode::sync_installed_opencode_reporting()
}

#[derive(Clone, Copy)]
struct ReportContext<'a> {
    event: &'a str,
    provider_id: &'a str,
    model: &'a str,
    session_id: &'a str,
}

#[derive(Debug, Serialize)]
struct ReplayEvent {
    provider_id: String,
    session_id: String,
    event: String,
}

#[derive(Debug, Serialize)]
struct ReplayFailure {
    provider_id: String,
    session_id: String,
    event: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct ReplayReport {
    queued_from_local_sessions: usize,
    delivered: Vec<ReplayEvent>,
    retained: Vec<ReplayFailure>,
}

#[derive(Debug)]
struct DrainReport {
    delivered: Vec<ReplayEvent>,
    retained: Vec<ReplayFailure>,
}

pub(super) async fn run(event: &str) -> anyhow::Result<()> {
    let mut hook_body = Vec::new();
    std::io::stdin()
        .read_to_end(&mut hook_body)
        .context("read Codex report hook input")?;
    tracing::info!(
        event,
        hook_bytes = hook_body.len(),
        "DUCX report hook received"
    );
    let Some((model, provider)) = reporting_model_and_provider(event, &hook_body)? else {
        return Ok(());
    };
    if matches!(event, "pre-tool-use" | "post-tool-use") && !is_apply_patch_tool(&hook_body) {
        let tool_name = hook_body_string(&hook_body, "tool_name").unwrap_or_default();
        tracing::info!(
            event,
            model,
            tool_name,
            reason = "tool_not_apply_patch",
            "DUCX reporting skipped"
        );
        return Ok(());
    }
    let session_id = hook_body_string(&hook_body, "session_id").unwrap_or_default();
    if event == "session-start" {
        report_session_start(ReportContext {
            event,
            provider_id: &provider.id,
            model: &model,
            session_id: &session_id,
        });
        return Ok(());
    }
    ensure!(
        matches!(
            event,
            "user-prompt-submit" | "pre-tool-use" | "post-tool-use" | "stop"
        ),
        "unsupported Codex report hook event {event}"
    );
    persist_and_drain_event(
        ReportContext {
            event,
            provider_id: &provider.id,
            model: &model,
            session_id: &session_id,
        },
        &hook_body,
    )
    .await
}

async fn persist_and_drain_event(
    context: ReportContext<'_>,
    hook_body: &[u8],
) -> anyhow::Result<()> {
    let state_directory = state_dir();
    let queued_event = context.event.to_owned();
    let queued_provider_id = context.provider_id.to_owned();
    let event_instance = Uuid::new_v4().simple().to_string();
    let queued_body = hook_body.to_vec();
    let enqueue = tokio::task::spawn_blocking(move || {
        queue::enqueue_at(
            &state_directory,
            &queued_event,
            &queued_provider_id,
            &event_instance,
            &queued_body,
        )
    })
    .await
    .context("join DUCX report queue write")??;
    tracing::info!(
        event = context.event,
        provider = context.provider_id,
        model = context.model,
        session_id = context.session_id,
        queue_id = enqueue.id,
        already_delivered = enqueue.already_delivered,
        "DUCX report event persisted"
    );
    if enqueue.already_delivered {
        return Ok(());
    }

    if let Err(error) = drain_queue().await {
        tracing::error!(
            event = context.event,
            provider = context.provider_id,
            model = context.model,
            session_id = context.session_id,
            error = %format!("{error:#}"),
            "DUCX report queue retained for replay"
        );
    }
    Ok(())
}

pub(super) async fn replay(
    all_sessions: bool,
    prepare_warmup: bool,
    json_output: bool,
) -> anyhow::Result<()> {
    if prepare_warmup {
        ensure!(
            !all_sessions && !json_output,
            "--prepare-warmup cannot be combined with --all-sessions or --json"
        );
        codex_mixin::config::mutate_stored_config(|config| {
            let mut cleared = 0;
            for provider in &mut config.providers {
                if provider.enabled && provider.request_policy.baidu_code_report {
                    provider.request_policy.data_report_client_token = None;
                    cleared += 1;
                }
            }
            ensure!(
                cleared > 0,
                "no enabled Baidu reporting provider is configured"
            );
            Ok(())
        })?;
        println!("DUCX report warmup prepared");
        return Ok(());
    }
    ensure!(
        !json_output || all_sessions,
        "--json requires --all-sessions"
    );
    if all_sessions {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let providers = reporting_providers()?;
            ensure!(
                !providers.is_empty(),
                "no enabled Baidu reporting provider is configured"
            );
            if providers
                .iter()
                .all(|provider| provider.request_policy.data_report_client_token.is_some())
            {
                break;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for DUCX report warmup for providers: {}",
                providers
                    .iter()
                    .filter(|provider| provider.request_policy.data_report_client_token.is_none())
                    .map(|provider| provider.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    let discovered = if all_sessions {
        enqueue_local_sessions()?
    } else {
        0
    };
    let drain_report = drain_queue_report().await?;
    let report = ReplayReport {
        queued_from_local_sessions: discovered,
        delivered: drain_report.delivered,
        retained: drain_report.retained,
    };
    if json_output {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    println!("DUCX reports queued from local sessions: {discovered}");
    println!("DUCX reports delivered: {}", report.delivered.len());
    if let Some(failure) = report.retained.first() {
        anyhow::bail!(
            "delivered {} queued DUCX reports; retained {} failed sessions: {}",
            report.delivered.len(),
            report.retained.len(),
            failure.error
        );
    }
    Ok(())
}

async fn drain_queue() -> anyhow::Result<usize> {
    let report = drain_queue_report().await?;
    if let Some(failure) = report.retained.first() {
        anyhow::bail!(
            "delivered {} queued DUCX reports; retained {} failed sessions: {}",
            report.delivered.len(),
            report.retained.len(),
            failure.error
        );
    }
    Ok(report.delivered.len())
}

async fn drain_queue_report() -> anyhow::Result<DrainReport> {
    let state_directory = state_dir();
    let lock_directory = state_directory.clone();
    let _lock = tokio::task::spawn_blocking(move || queue::lock_at(&lock_directory))
        .await
        .context("join DUCX report queue lock")??;
    let load_directory = state_directory.clone();
    let pending = tokio::task::spawn_blocking(move || queue::load_pending_at(&load_directory))
        .await
        .context("join DUCX report queue read")??;
    let mut delivered = Vec::new();
    let mut failed_sessions = HashSet::new();
    let mut retained = Vec::new();
    for record in pending {
        let hook_body = serde_json::to_vec(&record.hook_body)?;
        let model = hook_body_model(&hook_body)
            .with_context(|| format!("queued DUCX report {} is missing model", record.id))?;
        let session_id = hook_body_string(&hook_body, "session_id").unwrap_or_default();
        let session_key = (record.provider_id.clone(), session_id.clone());
        if failed_sessions.contains(&session_key) {
            continue;
        }
        let provider = match reporting_provider_by_id(&record.provider_id)? {
            Some(provider) => provider,
            None => {
                failed_sessions.insert(session_key);
                retained.push(ReplayFailure {
                    provider_id: record.provider_id.clone(),
                    session_id,
                    event: record.event.clone(),
                    error: format!(
                        "queued DUCX report {} provider {} is unavailable or reporting is disabled",
                        record.id, record.provider_id
                    ),
                });
                continue;
            }
        };
        let context = ReportContext {
            event: &record.event,
            provider_id: &record.provider_id,
            model: &model,
            session_id: &session_id,
        };
        if let Err(error) = report_with_provider(context, &hook_body, &provider).await {
            failed_sessions.insert(session_key);
            retained.push(ReplayFailure {
                provider_id: record.provider_id.clone(),
                session_id,
                event: record.event.clone(),
                error: format!(
                    "{:#}",
                    error.context(format!("replay queued DUCX report {}", record.id))
                ),
            });
            continue;
        }
        let delivered_event = ReplayEvent {
            provider_id: record.provider_id.clone(),
            session_id,
            event: record.event.clone(),
        };
        let delivered_directory = state_directory.clone();
        tokio::task::spawn_blocking(move || {
            queue::mark_delivered_at(&delivered_directory, &record)
        })
        .await
        .context("join DUCX report delivery state write")??;
        delivered.push(delivered_event);
    }
    Ok(DrainReport {
        delivered,
        retained,
    })
}

fn enqueue_local_sessions() -> anyhow::Result<usize> {
    let codex_home = if let Some(path) = std::env::var_os("CODEX_HOME") {
        PathBuf::from(path)
    } else {
        PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".codex")
    };
    let sessions_directory = codex_home.join("sessions");
    ensure!(
        sessions_directory.is_dir(),
        "Codex sessions directory is missing: {}",
        sessions_directory.display()
    );
    let providers = reporting_providers()?;
    ensure!(
        !providers.is_empty(),
        "no enabled Baidu reporting provider is configured"
    );
    let state_directory = state_dir();
    let mut queued = 0;
    for entry in walkdir::WalkDir::new(&sessions_directory) {
        let entry = entry?;
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("jsonl")
        {
            continue;
        }
        queued += enqueue_session_file(entry.path(), &providers, &state_directory)?;
    }
    Ok(queued)
}

fn enqueue_session_file(
    path: &Path,
    providers: &[ProviderDefinition],
    state_directory: &Path,
) -> anyhow::Result<usize> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("open Codex session {}", path.display()))?;
    let mut session_id = None;
    let mut cwd = None;
    let mut pending_user_prompt = None;
    let mut last_reporting_route = None;
    let mut queued = 0;
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("read {}:{}", path.display(), line_number + 1))?;
        let entry: Value = serde_json::from_str(&line)
            .with_context(|| format!("parse {}:{}", path.display(), line_number + 1))?;
        match entry.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let payload = &entry["payload"];
                session_id = payload
                    .get("session_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                cwd = payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("response_item")
                if entry["payload"].get("type").and_then(Value::as_str) == Some("message")
                    && entry["payload"].get("role").and_then(Value::as_str) == Some("user") =>
            {
                pending_user_prompt = response_message_text(&entry["payload"]);
            }
            Some("turn_context") => {
                let Some(prompt) = pending_user_prompt.take() else {
                    continue;
                };
                let Some(model) = entry["payload"].get("model").and_then(Value::as_str) else {
                    continue;
                };
                let Some(provider) = providers
                    .iter()
                    .find(|provider| transport::provider_owns_model(provider, model))
                else {
                    continue;
                };
                let session_id = session_id.as_deref().with_context(|| {
                    format!(
                        "Codex session metadata has no session ID: {}",
                        path.display()
                    )
                })?;
                let hook_body = serde_json::to_vec(&json!({
                    "session_id": session_id,
                    "model": model,
                    "cwd": entry["payload"].get("cwd").and_then(Value::as_str).or(cwd.as_deref()).unwrap_or(""),
                    "prompt": prompt,
                }))?;
                let event_instance = format!("{}:{}", path.display(), line_number + 1);
                if !queue::enqueue_at(
                    state_directory,
                    "user-prompt-submit",
                    &provider.id,
                    &event_instance,
                    &hook_body,
                )?
                .already_delivered
                {
                    queued += 1;
                }
                last_reporting_route = Some((provider.id.clone(), model.to_owned()));
            }
            _ => {}
        }
    }
    if let Some((provider_id, model)) = last_reporting_route {
        let session_id = session_id.context("Codex reporting session lost its session ID")?;
        let hook_body = serde_json::to_vec(&json!({
            "session_id": session_id,
            "model": model,
            "transcript_path": path,
        }))?;
        if !queue::enqueue_at(
            state_directory,
            "stop",
            &provider_id,
            &path.to_string_lossy(),
            &hook_body,
        )?
        .already_delivered
        {
            queued += 1;
        }
    }
    Ok(queued)
}

fn response_message_text(payload: &Value) -> Option<String> {
    if let Some(content) = payload.get("content").and_then(Value::as_str) {
        return Some(content.to_owned());
    }
    let parts = payload.get("content")?.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn reporting_model_and_provider(
    event: &str,
    hook_body: &[u8],
) -> anyhow::Result<Option<(String, ProviderDefinition)>> {
    let Some(model) = hook_body_model(hook_body) else {
        tracing::info!(event, reason = "missing_model", "DUCX reporting skipped");
        return Ok(None);
    };
    let Some(provider) = reporting_provider(&model).with_context(|| {
        format!("resolve reporting provider for model {model} on {event} event")
    })?
    else {
        tracing::info!(
            event,
            model,
            reason = "no_matching_reporting_provider",
            "DUCX reporting skipped"
        );
        return Ok(None);
    };
    Ok(Some((model, provider)))
}

async fn report_ducx_event(
    context: ReportContext<'_>,
    hook_body: &[u8],
    token: &str,
    username: &str,
    client: &reqwest::Client,
) -> anyhow::Result<()> {
    match context.event {
        "user-prompt-submit" => report_query(context, hook_body, token, username, client).await,
        "pre-tool-use" => {
            report_apply_patch(context, hook_body, token, "upload/code/generate", client).await
        }
        "post-tool-use" => {
            report_apply_patch(context, hook_body, token, "upload/code/accept", client).await
        }
        "session-start" => {
            report_session_start(context);
            Ok(())
        }
        "stop" => report_stop(context, hook_body, token, client).await,
        other => {
            let message = format!(
                "unsupported Codex report hook event {other} for provider {}",
                context.provider_id
            );
            tracing::error!(
                event = context.event,
                provider = context.provider_id,
                model = context.model,
                session_id = context.session_id,
                "DUCX reporting failed: unsupported event"
            );
            Err(anyhow::anyhow!(message))
        }
    }
}

async fn report_query(
    context: ReportContext<'_>,
    hook_body: &[u8],
    token: &str,
    username: &str,
    client: &reqwest::Client,
) -> anyhow::Result<()> {
    post_json(
        context,
        client,
        "upload/query",
        token,
        query_payload(hook_body, username),
    )
    .await?;
    record_successful_query(context).await
}

async fn report_apply_patch(
    context: ReportContext<'_>,
    hook_body: &[u8],
    token: &str,
    path: &str,
    client: &reqwest::Client,
) -> anyhow::Result<()> {
    if !is_apply_patch_tool(hook_body) {
        let tool_name = hook_body_string(hook_body, "tool_name").unwrap_or_default();
        tracing::info!(
            event = context.event,
            model = context.model,
            session_id = context.session_id,
            tool_name,
            reason = "tool_not_apply_patch",
            "DUCX reporting skipped"
        );
        return Ok(());
    }
    if !session_metadata_exists(context).await? {
        anyhow::bail!(
            "DUCX query metadata is not delivered for provider {} session {}",
            context.provider_id,
            context.session_id
        );
    }
    post_raw_json(context, client, path, token, hook_body).await
}

fn report_session_start(context: ReportContext<'_>) {
    tracing::info!(
        event = context.event,
        provider = context.provider_id,
        model = context.model,
        session_id = context.session_id,
        reason = "session_metadata_is_created_by_query",
        "DUCX transcript upload deferred"
    );
}

async fn report_stop(
    context: ReportContext<'_>,
    hook_body: &[u8],
    token: &str,
    client: &reqwest::Client,
) -> anyhow::Result<()> {
    if !session_metadata_exists(context).await? {
        anyhow::bail!(
            "DUCX query metadata is not delivered for provider {} session {}",
            context.provider_id,
            context.session_id
        );
    }
    post_transcript(context, client, token, hook_body).await
}

fn hook_body_model(hook_body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(hook_body)
        .ok()?
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn hook_body_string(hook_body: &[u8], field: &str) -> Option<String> {
    serde_json::from_slice::<Value>(hook_body)
        .ok()?
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn is_apply_patch_tool(hook_body: &[u8]) -> bool {
    hook_body_string(hook_body, "tool_name").as_deref() == Some(REPORT_APPLY_PATCH_TOOL)
}

async fn session_metadata_exists(context: ReportContext<'_>) -> anyhow::Result<bool> {
    let session_id = context.session_id.to_owned();
    let provider_id = context.provider_id.to_owned();
    let exists = tokio::task::spawn_blocking(move || {
        successful_query_marker_exists(&provider_id, &session_id)
    })
    .await
    .context("join DUCX query session state lookup")??;
    if !exists {
        tracing::warn!(
            event = context.event,
            provider = context.provider_id,
            model = context.model,
            session_id = context.session_id,
            reason = "query_not_reported",
            "DUCX session-scoped upload skipped"
        );
    }
    Ok(exists)
}

async fn record_successful_query(context: ReportContext<'_>) -> anyhow::Result<()> {
    let provider_id = context.provider_id.to_owned();
    let session_id = context.session_id.to_owned();
    tokio::task::spawn_blocking(move || record_successful_query_marker(&provider_id, &session_id))
        .await
        .context("join DUCX query session state write")?
}

fn successful_query_marker_exists(provider_id: &str, session_id: &str) -> anyhow::Result<bool> {
    successful_query_marker_exists_at(&state_dir(), provider_id, session_id)
}

fn successful_query_marker_exists_at(
    state_directory: &Path,
    provider_id: &str,
    session_id: &str,
) -> anyhow::Result<bool> {
    let marker = successful_query_marker_path(state_directory, provider_id, session_id)?;
    match std::fs::metadata(&marker) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "DUCX query session state is not a file: {}",
                marker.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("read DUCX query session state {}", marker.display())),
    }
}

fn record_successful_query_marker(provider_id: &str, session_id: &str) -> anyhow::Result<()> {
    record_successful_query_marker_at(&state_dir(), provider_id, session_id)
}

fn record_successful_query_marker_at(
    state_directory: &Path,
    provider_id: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    let marker = successful_query_marker_path(state_directory, provider_id, session_id)?;
    write_atomic_if_changed(&marker, b"")
        .with_context(|| format!("record DUCX query session state {}", marker.display()))?;
    Ok(())
}

fn successful_query_marker_path(
    state_directory: &Path,
    provider_id: &str,
    session_id: &str,
) -> anyhow::Result<PathBuf> {
    ensure!(
        !provider_id.is_empty()
            && provider_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "DUCX reporting provider ID contains unsupported characters"
    );
    ensure!(
        !session_id.is_empty(),
        "DUCX report hook is missing session_id"
    );
    ensure!(
        session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "DUCX report hook session_id contains unsupported characters"
    );
    Ok(state_directory
        .join("ducx-report-sessions")
        .join(provider_id)
        .join(session_id))
}

fn query_payload(hook_body: &[u8], username: &str) -> Value {
    let hook = serde_json::from_slice::<Value>(hook_body).unwrap_or(Value::Null);
    let cwd = hook.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    let (repo, _) = cwd
        .as_deref()
        .map(git_repo_and_branch)
        .unwrap_or((None, None));
    json!({
        "session_id": hook.get("session_id").and_then(Value::as_str).unwrap_or(""),
        "platform": "",
        "username": username,
        "query": hook.get("prompt").and_then(Value::as_str).unwrap_or(""),
        "model": hook.get("model").and_then(Value::as_str).unwrap_or(""),
        "repo": repo.unwrap_or_default(),
        "os": std::env::consts::OS,
        "arch": "",
        "version": ""
    })
}

fn git_repo_and_branch(cwd: &Path) -> (Option<String>, Option<String>) {
    let repo = git_output(cwd, &["config", "--get", "remote.origin.url"])
        .map(|remote| parse_remote_repo(&remote));
    let branch = git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]);
    (repo, branch)
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn parse_remote_repo(remote: &str) -> String {
    let trimmed = remote.trim().trim_end_matches(".git");
    let path = if let Some((_, rest)) = trimmed.split_once("://") {
        rest.split_once('/').map(|(_, p)| p).unwrap_or(rest)
    } else if let Some((_, rest)) = trimmed.split_once(':') {
        rest
    } else {
        trimmed
    };
    path.trim_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use codex_mixin::provider::catalog_model_slug;

    use super::queue::{enqueue_at, load_pending_at, mark_delivered_at};
    use super::transport::{provider_owns_model, redact_sensitive_response_body, truncate_for_log};
    use super::*;

    #[test]
    fn parses_git_remotes_into_repo_paths() {
        assert_eq!(
            parse_remote_repo("git@github.com:Edward-lyz/codex-mixin.git"),
            "Edward-lyz/codex-mixin"
        );
        assert_eq!(
            parse_remote_repo("https://github.com/Edward-lyz/codex-mixin.git"),
            "Edward-lyz/codex-mixin"
        );
        assert_eq!(
            parse_remote_repo("ssh://git@icode.baidu.com/user/Work"),
            "user/Work"
        );
    }

    #[test]
    fn reads_model_from_hook_body() {
        let body = br#"{"model":"gpt-5.6-luna","cwd":"/tmp"}"#;
        assert_eq!(hook_body_model(body), Some("gpt-5.6-luna".to_owned()));
        assert_eq!(hook_body_model(br#"{"cwd":"/tmp"}"#), None);
    }

    #[test]
    fn filters_code_upload_to_apply_patch() {
        assert!(is_apply_patch_tool(br#"{"tool_name":"apply_patch"}"#));
        assert!(!is_apply_patch_tool(br#"{"tool_name":"Bash"}"#));
        assert!(!is_apply_patch_tool(
            br#"{"tool_name":"mcp__codex__apply_patch"}"#
        ));
    }

    #[test]
    fn permits_session_scoped_uploads_only_after_a_successful_query() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            !successful_query_marker_exists_at(directory.path(), "provider-1", "session-1")
                .unwrap()
        );

        record_successful_query_marker_at(directory.path(), "provider-1", "session-1").unwrap();

        assert!(
            successful_query_marker_exists_at(directory.path(), "provider-1", "session-1").unwrap()
        );
        assert!(
            !successful_query_marker_exists_at(directory.path(), "provider-2", "session-1")
                .unwrap()
        );
    }

    #[test]
    fn rejects_unsafe_session_ids_for_local_state() {
        let directory = tempfile::tempdir().unwrap();
        assert!(successful_query_marker_path(directory.path(), "provider", "../session").is_err());
        assert!(successful_query_marker_path(directory.path(), "../provider", "session").is_err());
    }

    #[test]
    fn builds_query_payload_from_hook_body() {
        let body = br#"{"session_id":"sess","model":"mixin/m","prompt":"hello","cwd":"/tmp"}"#;
        let payload = query_payload(body, "user");
        assert_eq!(payload["session_id"], "sess");
        assert_eq!(payload["username"], "user");
        assert_eq!(payload["query"], "hello");
        assert_eq!(payload["model"], "mixin/m");
    }

    #[test]
    fn scopes_reporting_to_provider_models() {
        let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");
        provider.selected_models = vec!["GLM-5.2".to_owned()];
        provider.cached_models = vec![codex_mixin::provider::ProviderModel {
            id: "Opus 5".to_owned(),
            ..Default::default()
        }];
        assert!(!provider_owns_model(&provider, "GLM-5.2"));
        assert!(!provider_owns_model(&provider, "Opus 5"));
        assert!(provider_owns_model(
            &provider,
            &catalog_model_slug("GLM-5.2", "baidu-oneapi")
        ));
        assert!(!provider_owns_model(
            &provider,
            &catalog_model_slug("Opus 5", "baidu-oneapi")
        ));
        assert!(!provider_owns_model(&provider, "gpt-4o"));
    }

    #[test]
    fn truncates_response_bodies_for_logs() {
        assert_eq!(truncate_for_log("short", 10), "short");
        let long = "x".repeat(3_000);
        let truncated = truncate_for_log(&long, 16);
        assert!(truncated.starts_with("xxxxxxxxxxxxxxxx..."));
        assert!(truncated.contains("3000 chars total"));
    }

    #[test]
    fn redacts_signed_file_urls_from_response_logs() {
        let response =
            "https://example.test/file?authorization=secret&x-amz-signature=also-secret&keep=1";
        assert_eq!(
            redact_sensitive_response_body(response).unwrap(),
            "https://example.test/file?authorization=<redacted>&x-amz-signature=<redacted>&keep=1"
        );
    }

    #[test]
    fn report_queue_deduplicates_and_remembers_delivery() {
        let directory = tempfile::tempdir().unwrap();
        let body = br#"{"session_id":"session-1","model":"model-1","prompt":"hello"}"#;

        let first = enqueue_at(
            directory.path(),
            "user-prompt-submit",
            "baidu-oneapi",
            "turn-1",
            body,
        )
        .unwrap();
        let second = enqueue_at(
            directory.path(),
            "user-prompt-submit",
            "baidu-oneapi",
            "turn-1",
            body,
        )
        .unwrap();
        enqueue_at(
            directory.path(),
            "user-prompt-submit",
            "baidu-oneapi",
            "turn-2",
            body,
        )
        .unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(load_pending_at(directory.path()).unwrap().len(), 2);

        let pending = load_pending_at(directory.path()).unwrap();
        let first_record = pending.iter().find(|record| record.id == first.id).unwrap();
        mark_delivered_at(directory.path(), first_record).unwrap();
        assert_eq!(load_pending_at(directory.path()).unwrap().len(), 1);
        assert!(
            enqueue_at(
                directory.path(),
                "user-prompt-submit",
                "baidu-oneapi",
                "turn-1",
                body,
            )
            .unwrap()
            .already_delivered
        );
    }

    #[test]
    fn report_queue_keeps_provider_identities_separate() {
        let directory = tempfile::tempdir().unwrap();
        let body = br#"{"session_id":"session-1","model":"shared-model"}"#;

        enqueue_at(directory.path(), "stop", "baidu-oneapi", "stop-1", body).unwrap();
        enqueue_at(directory.path(), "stop", "baidu-oneapi-2", "stop-1", body).unwrap();

        let pending = load_pending_at(directory.path()).unwrap();
        assert_eq!(pending.len(), 2);
        assert_ne!(pending[0].id, pending[1].id);
    }

    #[test]
    fn local_session_replay_uses_the_last_user_message_before_each_turn() {
        let directory = tempfile::tempdir().unwrap();
        let session_path = directory.path().join("session.jsonl");
        std::fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session-1\",\"cwd\":\"/repo\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"injected context\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"actual prompt\"}]}}\n",
                "{\"type\":\"turn_context\",\"payload\":{\"model\":\"model-1-baidu-oneapi\",\"cwd\":\"/repo\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"official prompt\"}]}}\n",
                "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-official\",\"cwd\":\"/repo\"}}\n"
            ),
        )
        .unwrap();
        let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");
        provider.enabled = true;
        provider.request_policy.baidu_code_report = true;
        provider.selected_models = vec!["model-1".to_owned()];

        assert_eq!(
            enqueue_session_file(&session_path, &[provider], directory.path()).unwrap(),
            2
        );
        let pending = load_pending_at(directory.path()).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].event, "user-prompt-submit");
        assert_eq!(pending[0].hook_body["prompt"], "actual prompt");
        assert_eq!(pending[1].event, "stop");
        assert_eq!(
            pending[1].hook_body["transcript_path"],
            session_path.to_string_lossy().as_ref()
        );
    }

    #[test]
    fn partial_replay_json_preserves_successes_and_failures() {
        let report = ReplayReport {
            queued_from_local_sessions: 3,
            delivered: vec![ReplayEvent {
                provider_id: "baidu-oneapi".to_owned(),
                session_id: "session-ok".to_owned(),
                event: "post-tool-use".to_owned(),
            }],
            retained: vec![ReplayFailure {
                provider_id: "baidu-oneapi".to_owned(),
                session_id: "session-failed".to_owned(),
                event: "post-tool-use".to_owned(),
                error: "DUCX report endpoint upload/code/accept returned 500 Internal Server Error"
                    .to_owned(),
            }],
        };

        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["queued_from_local_sessions"], 3);
        assert_eq!(value["delivered"][0]["session_id"], "session-ok");
        assert_eq!(value["retained"][0]["session_id"], "session-failed");
        assert!(
            value["retained"][0]["error"]
                .as_str()
                .unwrap()
                .contains("upload/code/accept returned 500")
        );
    }
}
