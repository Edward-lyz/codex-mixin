//! Mixin-level Baidu code-usage reporting.
//!
//! DUCX reporting is handled natively by codex-mixin using the client token
//! captured during DUCX warmup.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use anyhow::{Context, ensure};
use serde_json::{Value, json};

use crate::cli::atomic_file::write_atomic_if_changed;
use crate::cli::runtime::state_dir;
use codex_mixin::provider::ProviderDefinition;

mod installation;
mod transport;

pub(super) use installation::{reporting_enabled, sync_installation, sync_installation_at};
use transport::{
    post_json, post_raw_json, post_transcript, report_with_provider, reporting_provider,
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

#[derive(Clone, Copy)]
struct ReportContext<'a> {
    event: &'a str,
    provider_id: &'a str,
    model: &'a str,
    session_id: &'a str,
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
    let session_id = hook_body_string(&hook_body, "session_id").unwrap_or_default();
    let context = ReportContext {
        event,
        provider_id: &provider.id,
        model: &model,
        session_id: &session_id,
    };
    report_with_provider(context, &hook_body, &provider).await
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
    record_successful_query(context.session_id).await
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
        return Ok(());
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
        return Ok(());
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
    let exists = tokio::task::spawn_blocking(move || successful_query_marker_exists(&session_id))
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

async fn record_successful_query(session_id: &str) -> anyhow::Result<()> {
    let session_id = session_id.to_owned();
    tokio::task::spawn_blocking(move || record_successful_query_marker(&session_id))
        .await
        .context("join DUCX query session state write")?
}

fn successful_query_marker_exists(session_id: &str) -> anyhow::Result<bool> {
    successful_query_marker_exists_at(&state_dir(), session_id)
}

fn successful_query_marker_exists_at(
    state_directory: &Path,
    session_id: &str,
) -> anyhow::Result<bool> {
    let marker = successful_query_marker_path(state_directory, session_id)?;
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

fn record_successful_query_marker(session_id: &str) -> anyhow::Result<()> {
    record_successful_query_marker_at(&state_dir(), session_id)
}

fn record_successful_query_marker_at(
    state_directory: &Path,
    session_id: &str,
) -> anyhow::Result<()> {
    let marker = successful_query_marker_path(state_directory, session_id)?;
    write_atomic_if_changed(&marker, b"")
        .with_context(|| format!("record DUCX query session state {}", marker.display()))?;
    Ok(())
}

fn successful_query_marker_path(
    state_directory: &Path,
    session_id: &str,
) -> anyhow::Result<PathBuf> {
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
        assert!(!successful_query_marker_exists_at(directory.path(), "session-1").unwrap());

        record_successful_query_marker_at(directory.path(), "session-1").unwrap();

        assert!(successful_query_marker_exists_at(directory.path(), "session-1").unwrap());
    }

    #[test]
    fn rejects_unsafe_session_ids_for_local_state() {
        let directory = tempfile::tempdir().unwrap();
        assert!(successful_query_marker_path(directory.path(), "../session").is_err());
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
}
