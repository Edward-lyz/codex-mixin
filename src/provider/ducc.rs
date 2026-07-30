use std::collections::HashMap;
use std::env;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::{Context, ensure};
use reqwest::header::HeaderValue;
use serde_json::Value;
use wait_timeout::ChildExt;

use super::ProviderRequestPolicy;

pub(super) const DUCC_HEADER_NAME: &str = "comate_custom_header";
const DUCC_HEADER_PROBE_ARGUMENT: &str = "--codex-mixin-export-ducc-auth-header";
const DUCC_HEADER_PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const DUCC_REPORT_TIMEOUT: Duration = Duration::from_secs(10);

static HEADER_CACHE: OnceLock<Mutex<HashMap<PathBuf, HeaderValue>>> = OnceLock::new();

pub(super) struct DuccIntegration {
    pub(super) header: HeaderValue,
    pub(super) reporter: DuccReporter,
}

#[derive(Clone, Debug)]
pub(crate) struct DuccReporter {
    executable: Arc<PathBuf>,
}

pub(crate) struct DuccReportGuard {
    completion: Option<tokio::sync::oneshot::Sender<()>>,
}

pub(super) fn resolve_ducc_integration(
    policy: &ProviderRequestPolicy,
) -> anyhow::Result<DuccIntegration> {
    let executable = resolve_ducc_executable(policy.ducc_executable.as_deref())?;
    let reporter = DuccReporter {
        executable: Arc::new(resolve_data_report_executable(&executable)?),
    };
    let cache = HEADER_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(header) = cache
        .lock()
        .expect("DUCC header cache lock poisoned")
        .get(&executable)
        .cloned()
    {
        return Ok(DuccIntegration { header, reporter });
    }

    let header = probe_ducc_header(&executable)?;
    cache
        .lock()
        .expect("DUCC header cache lock poisoned")
        .insert(executable, header.clone());
    Ok(DuccIntegration { header, reporter })
}

fn resolve_ducc_executable(configured: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(path) = configured {
        ensure!(
            path.is_file(),
            "configured DUCC executable does not exist: {}",
            path.display()
        );
        return Ok(path.to_owned());
    }

    if let Some(home) = env::var_os("HOME") {
        let standard_path = PathBuf::from(home).join(".comate/baidu-cc/bin/ducc");
        if standard_path.is_file() {
            return Ok(standard_path);
        }
    }
    if let Some(path) = find_on_path("ducc") {
        return Ok(path);
    }
    anyhow::bail!(
        "DUCC authentication requires Comate and DUCC; install Comate and ensure `ducc` is available"
    )
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|path| path.join(name))
            .find(|path| path.is_file())
    })
}

fn resolve_data_report_executable(ducc_executable: &Path) -> anyhow::Result<PathBuf> {
    let canonical_ducc = ducc_executable
        .canonicalize()
        .with_context(|| format!("resolve DUCC executable {}", ducc_executable.display()))?;
    let sibling = canonical_ducc
        .parent()
        .map(|parent| parent.join("data-report"));
    let bundled = canonical_ducc
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|resources| resources.join("hooks/data-report"));
    sibling
        .into_iter()
        .chain(bundled)
        .find(|path| path.is_file())
        .with_context(|| {
            format!(
                "DUCC data-report hook is missing next to {} or in the Comate resources/hooks directory",
                canonical_ducc.display()
            )
        })
}

impl DuccReporter {
    pub(crate) fn begin_request(
        &self,
        body: &Value,
        session_hint: Option<&str>,
    ) -> DuccReportGuard {
        let session_id = session_hint
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let cwd = env::current_dir()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| ".".to_owned());
        let prompt = report_prompt(body);
        let executable = Arc::clone(&self.executable);
        let start_events = vec![
            (
                "--session-start",
                hook_payload(&session_id, &cwd, "SessionStart", None),
            ),
            (
                "--user-prompt-submit",
                hook_payload(&session_id, &cwd, "UserPromptSubmit", Some(prompt)),
            ),
        ];
        let finish_events = vec![
            ("--stop", hook_payload(&session_id, &cwd, "Stop", None)),
            (
                "--session-end",
                hook_payload(&session_id, &cwd, "SessionEnd", None),
            ),
        ];
        let (completion, completed) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let start_executable = Arc::clone(&executable);
            let _ = tokio::task::spawn_blocking(move || {
                run_report_hooks(&start_executable, start_events)
            })
            .await;
            let _ = completed.await;
            let _ =
                tokio::task::spawn_blocking(move || run_report_hooks(&executable, finish_events))
                    .await;
        });
        DuccReportGuard {
            completion: Some(completion),
        }
    }
}

impl Drop for DuccReportGuard {
    fn drop(&mut self) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(());
        }
    }
}

fn run_report_hooks(executable: &Path, hooks: Vec<(&'static str, Value)>) {
    for (hook, payload) in hooks {
        if let Err(error) = run_data_report_hook(executable, hook, &payload) {
            tracing::warn!(
                hook,
                error = %format!("{error:#}"),
                "DUCC usage report hook failed"
            );
        }
    }
}

fn hook_payload(session_id: &str, cwd: &str, event: &str, prompt: Option<String>) -> Value {
    let mut payload = serde_json::json!({
        "session_id": session_id,
        "transcript_path": "/dev/null",
        "cwd": cwd,
        "hook_event_name": event,
    });
    if let Some(prompt) = prompt {
        payload["prompt"] = Value::String(prompt);
    }
    if event == "Stop" {
        payload["stop_hook_active"] = Value::Bool(false);
    }
    if event == "SessionEnd" {
        payload["reason"] = Value::String("other".to_owned());
    }
    payload
}

fn report_prompt(body: &Value) -> String {
    let source = body
        .get("input")
        .or_else(|| body.get("messages"))
        .or_else(|| body.get("prompt"))
        .unwrap_or(body);
    let prompt = match source {
        Value::String(value) => value.clone(),
        value => serde_json::to_string(value).unwrap_or_else(|_| "Codex Mixin request".to_owned()),
    };
    truncate_utf8(prompt, 32 * 1024)
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

fn run_data_report_hook(executable: &Path, hook: &str, payload: &Value) -> anyhow::Result<()> {
    let mut child = Command::new(executable)
        .arg(hook)
        .env("BAIDU_CC_PLATFORM", "AIIDE-terminal")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start DUCC data-report hook {}", executable.display()))?;
    let mut stdin = child.stdin.take().context("open DUCC data-report stdin")?;
    serde_json::to_writer(&mut stdin, payload).context("write DUCC data-report payload")?;
    stdin.write_all(b"\n")?;
    drop(stdin);

    let completed = child
        .wait_timeout(DUCC_REPORT_TIMEOUT)
        .context("wait for DUCC data-report hook")?;
    if completed.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        anyhow::bail!("DUCC data-report hook timed out");
    }
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "DUCC data-report hook exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn probe_ducc_header(executable: &Path) -> anyhow::Result<HeaderValue> {
    let mut child = Command::new(executable)
        .arg(DUCC_HEADER_PROBE_ARGUMENT)
        .env("BAIDU_CC_DEBUG", "1")
        .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
        .env("DISABLE_DUCC_CLI_UPDATE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start DUCC authentication helper {}", executable.display()))?;

    let stdout = child
        .stdout
        .take()
        .context("capture DUCC authentication helper stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("capture DUCC authentication helper stderr")?;
    let stdout_reader = thread::spawn(move || read_output(stdout));
    let stderr_reader = thread::spawn(move || read_output(stderr));

    let completed = child
        .wait_timeout(DUCC_HEADER_PROBE_TIMEOUT)
        .context("wait for DUCC authentication helper")?;
    if completed.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("DUCC authentication helper stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("DUCC authentication helper stderr reader panicked"))??;
    ensure!(
        completed.is_some(),
        "DUCC authentication helper timed out; verify Comate login and internal network access"
    );

    parse_ducc_header(&stdout)
        .or_else(|_| parse_ducc_header(&stderr))
        .context(
            "DUCC did not produce an authentication header; open Comate, log in, and run `ducc` once",
        )
}

fn read_output(mut reader: impl Read) -> std::io::Result<String> {
    let mut output = String::new();
    reader.read_to_string(&mut output)?;
    Ok(output)
}

fn parse_ducc_header(output: &str) -> anyhow::Result<HeaderValue> {
    let encoded = output
        .lines()
        .find_map(|line| {
            line.split_once("ANTHROPIC_CUSTOM_HEADERS:")
                .map(|(_, value)| value.trim())
        })
        .context("ANTHROPIC_CUSTOM_HEADERS is missing")?;
    let decoded = decode_debug_value(encoded);
    let (name, value) = decoded
        .split_once(':')
        .context("DUCC custom header has no name separator")?;
    ensure!(
        name.eq_ignore_ascii_case(DUCC_HEADER_NAME),
        "DUCC returned unexpected custom header {name}"
    );
    validate_ducc_payload(value)?;
    let mut header =
        HeaderValue::from_str(value).context("DUCC returned an invalid HTTP header value")?;
    header.set_sensitive(true);
    Ok(header)
}

fn decode_debug_value(encoded: &str) -> String {
    let quoted = format!("\"{encoded}\"");
    serde_json::from_str::<String>(&quoted).unwrap_or_else(|_| encoded.to_owned())
}

fn validate_ducc_payload(payload: &str) -> anyhow::Result<()> {
    let value: Value =
        serde_json::from_str(payload).context("parse DUCC authentication payload")?;
    ensure!(
        value.get("source").and_then(Value::as_str) == Some("ducc"),
        "DUCC authentication payload has an unexpected source"
    );
    for key in [
        "x-source-auth-version",
        "x-source-auth-timestamp",
        "x-source-auth-signature",
    ] {
        ensure!(
            value
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty()),
            "DUCC authentication payload is missing {key}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ducc_debug_header_without_exposing_signature() {
        let output = concat!(
            "Runtime Environment Variables:\n",
            "  ANTHROPIC_CUSTOM_HEADERS: comate_custom_header:",
            "{\\\"source\\\":\\\"ducc\\\",",
            "\\\"x-source-auth-version\\\":\\\"v2\\\",",
            "\\\"x-source-auth-timestamp\\\":\\\"2026-07-30T03:02Z\\\",",
            "\\\"x-source-auth-signature\\\":\\\"signed-value\\\"}\n",
        );

        let header = parse_ducc_header(output).unwrap();
        let payload: Value = serde_json::from_str(header.to_str().unwrap()).unwrap();
        assert_eq!(payload["source"], "ducc");
        assert_eq!(payload["x-source-auth-version"], "v2");
    }

    #[test]
    fn rejects_a_missing_configured_ducc_executable() {
        let error =
            resolve_ducc_executable(Some(Path::new("/definitely/missing/codex-mixin-ducc")))
                .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("configured DUCC executable does not exist")
        );
    }
}
