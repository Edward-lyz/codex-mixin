use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};

use codex_mixin::config::load_stored_config;
use codex_mixin::provider::{BaiduAuthBridge, ProviderDefinition, catalog_model_slug};

use super::{ReportContext, hook_body_string, report_ducx_event};

const DUCX_REPORT_BASE_URL: &str = "http://ducc-data.baidu-int.com:8501/api/rest/v1/ducx";
const REPORT_CLIENT_TOKEN_HEADER: &str = "x-auth-client-token";

pub(super) async fn report_with_provider(
    context: ReportContext<'_>,
    hook_body: &[u8],
    provider: &ProviderDefinition,
) -> anyhow::Result<()> {
    if provider.request_policy.effective_baidu_auth_bridge() == BaiduAuthBridge::Disabled {
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
        anyhow::bail!(message);
    }
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
    let home = ducx_report_home(provider)?;
    let username = managed_username(&home)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("build DUCX report client")?;
    report_ducx_event(context, hook_body, token, &username, &client).await
}

pub(super) async fn post_json(
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

pub(super) async fn post_raw_json(
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

pub(super) async fn post_transcript(
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

pub(super) fn truncate_for_log(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}... ({} chars total)", value.chars().count())
}

/// Select the enabled Baidu reporting provider that owns `model`.
pub(super) fn reporting_provider(model: &str) -> anyhow::Result<Option<ProviderDefinition>> {
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
pub(super) fn provider_owns_model(
    provider: &codex_mixin::provider::ProviderDefinition,
    model: &str,
) -> bool {
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
        .filter_map(Result::ok)
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
