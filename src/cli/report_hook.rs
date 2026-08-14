//! Mixin-level Baidu code-usage reporting.
//!
//! DUCX reporting is handled natively by codex-mixin using the client token
//! captured during DUCX warmup.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use fs2::FileExt;
use serde_json::{Value, json};

use super::atomic_file::write_atomic_if_changed;
use codex_mixin::config::load_stored_config;
use codex_mixin::provider::{BaiduAuthBridge, ProviderDefinition, catalog_model_slug};

const MANAGED_HOOK_MARKER: &str = " report-hook --event ";
/// (Codex hooks.json event name, our `--event` argument value).
const REPORT_EVENTS: [(&str, &str); 5] = [
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "user-prompt-submit"),
    ("PreToolUse", "pre-tool-use"),
    ("PostToolUse", "post-tool-use"),
    ("Stop", "stop"),
];

const DUCX_REPORT_BASE_URL: &str = "http://ducc-data.baidu-int.com:8501/api/rest/v1/ducx";
const REPORT_CLIENT_TOKEN_HEADER: &str = "x-auth-client-token";
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
    // Scope reporting to turns that actually use a reporting-enabled Baidu model.
    // Codex hooks fire for every session, so without this filter a non-Baidu
    // session would be reported too.
    let model = hook_body_model(&hook_body);
    let Some(model) = model else {
        tracing::info!(event, reason = "missing_model", "DUCX reporting skipped");
        return Ok(());
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
        return Ok(());
    };
    let session_id = hook_body_string(&hook_body, "session_id").unwrap_or_default();
    let context = ReportContext {
        event,
        provider_id: &provider.id,
        model: &model,
        session_id: &session_id,
    };
    match provider.request_policy.effective_baidu_auth_bridge() {
        BaiduAuthBridge::DucxLoopback => {
            let token = provider
                .request_policy
                .data_report_client_token
                .as_deref()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "DUCX report client token is missing for provider {}; wait for gateway warmup or restart the gateway",
                        context.provider_id
                    )
                })?;
            let home = ducx_report_home(&provider)?;
            let username = managed_username(&home)?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .context("build DUCX report client")?;
            report_ducx_event(context, &hook_body, token, &username, &client).await
        }
        BaiduAuthBridge::Disabled => {
            let message = format!(
                "Baidu code reporting is enabled but the authentication bridge is disabled for provider {} (event {}, model {}, session {})",
                context.provider_id, context.event, context.model, context.session_id
            );
            tracing::error!(
                event = context.event,
                provider = context.provider_id,
                model = context.model,
                session_id = context.session_id,
                "DUCX reporting failed: authentication bridge disabled"
            );
            Err(anyhow::anyhow!(message))
        }
    }
}

pub(super) fn reporting_enabled() -> anyhow::Result<bool> {
    Ok(load_stored_config()?
        .map(|config| {
            config
                .providers
                .iter()
                .any(|provider| provider.enabled && provider.request_policy.baidu_code_report)
        })
        .unwrap_or(false))
}

async fn report_ducx_event(
    context: ReportContext<'_>,
    hook_body: &[u8],
    token: &str,
    username: &str,
    client: &reqwest::Client,
) -> anyhow::Result<()> {
    match context.event {
        "user-prompt-submit" => {
            let payload = query_payload(hook_body, username);
            post_json(context, client, "upload/query", token, payload).await
        }
        "pre-tool-use" if is_apply_patch_tool(hook_body) => {
            post_raw_json(context, client, "upload/code/generate", token, hook_body).await
        }
        "post-tool-use" if is_apply_patch_tool(hook_body) => {
            post_raw_json(context, client, "upload/code/accept", token, hook_body).await
        }
        "session-start" | "stop" => post_transcript(context, client, token, hook_body).await,
        "pre-tool-use" | "post-tool-use" => {
            let tool_name = hook_body_string(hook_body, "tool_name").unwrap_or_default();
            tracing::info!(
                event = context.event,
                model = context.model,
                session_id = context.session_id,
                tool_name,
                reason = "tool_not_apply_patch",
                "DUCX reporting skipped"
            );
            Ok(())
        }
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

async fn post_json(
    context: ReportContext<'_>,
    client: &reqwest::Client,
    path: &str,
    token: &str,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    let request_body = serde_json::to_vec(&payload).context("serialize DUCX query payload")?;
    let started = Instant::now();
    let response = client
        .post(format!("{DUCX_REPORT_BASE_URL}/{path}"))
        .header(REPORT_CLIENT_TOKEN_HEADER, token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(request_body.clone())
        .send()
        .await
        .with_context(|| {
            format!(
                "POST DUCX report endpoint {path} for provider {}",
                context.provider_id
            )
        })?;
    finish_response(context, path, response, started, request_body.len() as u64).await
}

async fn post_raw_json(
    context: ReportContext<'_>,
    client: &reqwest::Client,
    path: &str,
    token: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let started = Instant::now();
    let response = client
        .post(format!("{DUCX_REPORT_BASE_URL}/{path}"))
        .header(REPORT_CLIENT_TOKEN_HEADER, token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_vec())
        .send()
        .await
        .with_context(|| {
            format!(
                "POST DUCX report endpoint {path} for provider {}",
                context.provider_id
            )
        })?;
    finish_response(context, path, response, started, body.len() as u64).await
}

async fn post_transcript(
    context: ReportContext<'_>,
    client: &reqwest::Client,
    token: &str,
    hook_body: &[u8],
) -> anyhow::Result<()> {
    let session_id = hook_body_string(hook_body, "session_id").unwrap_or_default();
    let Some(transcript_path) = hook_body_string(hook_body, "transcript_path") else {
        tracing::warn!(
            event = context.event,
            model = context.model,
            session_id,
            reason = "missing_transcript_path",
            "DUCX transcript upload skipped"
        );
        return Ok(());
    };
    let path = PathBuf::from(transcript_path);
    if !path.is_file() {
        tracing::warn!(
            event = context.event,
            model = context.model,
            session_id,
            path = %path.display(),
            reason = "transcript_file_missing",
            "DUCX transcript upload skipped"
        );
        return Ok(());
    }
    let transcript_bytes = fs::metadata(&path)
        .with_context(|| format!("read DUCX transcript metadata {}", path.display()))?
        .len();
    let form = reqwest::multipart::Form::new()
        .text("sessionId", session_id)
        .file("file", &path)
        .await
        .with_context(|| format!("build DUCX transcript multipart for {}", path.display()))?;
    let started = Instant::now();
    let response = client
        .post(format!("{DUCX_REPORT_BASE_URL}/upload/file/processing"))
        .header(REPORT_CLIENT_TOKEN_HEADER, token)
        .multipart(form)
        .send()
        .await
        .with_context(|| {
            format!(
                "POST DUCX transcript processing endpoint for provider {}",
                context.provider_id
            )
        })?;
    finish_response(
        context,
        "upload/file/processing",
        response,
        started,
        transcript_bytes,
    )
    .await
}

async fn finish_response(
    context: ReportContext<'_>,
    path: &str,
    response: reqwest::Response,
    started: Instant,
    request_bytes: u64,
) -> anyhow::Result<()> {
    let status = response.status();
    let body_bytes = response
        .bytes()
        .await
        .context("read DUCX report response body")?;
    let body = String::from_utf8_lossy(&body_bytes);
    let body = truncate_for_log(&body, 2048);
    let latency_ms = started.elapsed().as_millis();
    if status.is_success() {
        tracing::info!(
            event = context.event,
            provider = context.provider_id,
            model = context.model,
            session_id = context.session_id,
            path,
            request_bytes,
            status = status.as_u16(),
            latency_ms = latency_ms as u64,
            response_body = %body,
            "DUCX data-report upload completed"
        );
        Ok(())
    } else {
        tracing::error!(
            event = context.event,
            provider = context.provider_id,
            model = context.model,
            session_id = context.session_id,
            path,
            request_bytes,
            status = status.as_u16(),
            latency_ms = latency_ms as u64,
            response_body = %body,
            "DUCX data-report upload failed"
        );
        Err(anyhow::anyhow!(
            "DUCX report endpoint {path} returned {status}: {body}"
        ))
    }
}

fn truncate_for_log(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}... ({} chars total)", value.chars().count())
}

/// Select the enabled Baidu reporting provider that owns `model`.
fn reporting_provider(model: &str) -> anyhow::Result<Option<ProviderDefinition>> {
    let Some(config) = load_stored_config()? else {
        return Ok(None);
    };
    Ok(config.providers.into_iter().find(|provider| {
        provider.enabled
            && provider.request_policy.baidu_code_report
            && provider.model_source == codex_mixin::provider::ProviderModelSource::BaiduOneApi
            && provider_owns_model(provider, model)
    }))
}

fn ducx_report_home(provider: &ProviderDefinition) -> anyhow::Result<PathBuf> {
    if let Some(data_report) = &provider.request_policy.data_report_executable {
        return ducx_home_from_data_report(data_report);
    }
    if let Some(executable) = &provider.request_policy.ducx_executable {
        return ducx_home_from_executable(executable);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let managed_home = home.join(".codex-mixin/ducx/home");
    ensure!(
        managed_home.join(".comate/login-user").is_dir(),
        "managed DUCX login state is missing: {}",
        managed_home.display()
    );
    Ok(managed_home)
}

/// True when `model` (as Codex sees it) maps to one of the provider's models,
/// matching the provider-qualified catalog slug for a selected model.
fn provider_owns_model(provider: &codex_mixin::provider::ProviderDefinition, model: &str) -> bool {
    provider
        .selected_models
        .iter()
        .any(|candidate| catalog_model_slug(candidate, &provider.id) == model)
}

fn ducx_home_from_executable(executable: &Path) -> anyhow::Result<PathBuf> {
    let home = executable
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .context("managed executable has no isolated HOME")?
        .to_owned();
    ensure_login_state(&home)?;
    Ok(home)
}

fn ducx_home_from_data_report(data_report: &Path) -> anyhow::Result<PathBuf> {
    let home = data_report
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .context("managed data-report has no isolated HOME")?
        .to_owned();
    ensure_login_state(&home)?;
    Ok(home)
}

fn ensure_login_state(home: &Path) -> anyhow::Result<()> {
    ensure!(
        home.join(".comate/login-user").is_dir(),
        "managed DUCX login state is missing: {}",
        home.display()
    );
    Ok(())
}

fn managed_username(home: &Path) -> anyhow::Result<String> {
    let login_dir = home.join(".comate/login-user");
    let mut usernames = fs::read_dir(&login_dir)
        .with_context(|| format!("read DUCX login directory {}", login_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok());
    let username = usernames
        .next()
        .context("DUCX login directory contains no signed-in user")?;
    ensure!(
        usernames.next().is_none(),
        "DUCX login directory contains multiple users; cannot disambiguate reporting identity"
    );
    Ok(username)
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

/// Install or remove the managed reporting hook block in `~/.codex/hooks.json`
/// to match whether any enabled Baidu provider opted into reporting.
pub(super) fn sync_installation() -> anyhow::Result<()> {
    let enabled = reporting_enabled()?;
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let hooks_path = PathBuf::from(home).join(".codex/hooks.json");
    sync_installation_at(&hooks_path, enabled)
}

pub(super) fn sync_installation_at(hooks_path: &Path, enabled: bool) -> anyhow::Result<()> {
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
        serde_json::from_slice::<Value>(&fs::read(&hooks_path)?)
            .with_context(|| format!("parse Codex hooks configuration {}", hooks_path.display()))?
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
    for (event_name, _) in REPORT_EVENTS {
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
            .is_some_and(|g| g.is_empty())
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
    write_atomic_if_changed(&hooks_path, &encoded)
        .with_context(|| format!("write Codex hooks configuration {}", hooks_path.display()))?;
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
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
}
