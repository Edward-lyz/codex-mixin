//! Bounded, read-only reports from the signed-in Antigravity CLI.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, ensure};
use tokio::io::{AsyncRead, AsyncReadExt};

const REPORT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REPORT_BYTES: usize = 1024 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum AgyReport {
    Models,
    Usage,
}

struct ReportChild {
    child: tokio::process::Child,
    pid: Option<u32>,
}

impl Drop for ReportChild {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            #[cfg(unix)]
            let _ = super::kill_process_tree(pid);
            #[cfg(not(unix))]
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = super::kill_process_tree(pid);
            }
        }
    }
}

pub(crate) async fn agy_report(
    report: AgyReport,
    token_path: Option<PathBuf>,
) -> anyhow::Result<serde_json::Value> {
    let executable = tokio::task::spawn_blocking(|| super::agent_cli::find_command("agy"))
        .await?
        .context("Antigravity CLI is not installed; install agy and sign in first")?;
    read_report(&executable, report, token_path).await
}

async fn read_report(
    executable: &Path,
    report: AgyReport,
    token_path: Option<PathBuf>,
) -> anyhow::Result<serde_json::Value> {
    let directory = tokio::task::spawn_blocking(move || {
        let directory = prepare_report_directory()?;
        if let Some(path) = token_path {
            let mut file =
                std::fs::File::open(path).context("open AGY credentials for scoped report")?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(&mut file, 65537), &mut bytes)?;
            ensure!(bytes.len() <= 65536, "AGY credential file is too large");
            let gemini = directory.path().join(".gemini");
            let auth = gemini.join("antigravity-cli");
            std::fs::create_dir_all(&auth)?;
            super::set_owner_only_dir_mode(&gemini)?;
            super::set_owner_only_dir_mode(&auth)?;
            let staged = auth.join("antigravity-oauth-token");
            std::fs::write(&staged, bytes)?;
            super::set_owner_only_mode(&staged)?;
        }
        Ok::<_, anyhow::Error>(directory)
    })
    .await??;
    let version = run(executable, &["--version"], directory.path()).await?;
    let version: Vec<_> = std::str::from_utf8(&version)?
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<_, _>>()
        .context("AGY returned an invalid version")?;
    ensure!(
        version.len() == 3 && version.as_slice() >= [1, 3, 5].as_slice(),
        "AGY 1.3.5 or newer is required for the verified read-only JSON commands"
    );
    let arguments: &[&str] = match report {
        AgyReport::Models => &["--output-format", "json", "models"],
        AgyReport::Usage => &[
            "--sandbox",
            "-p",
            "/usage",
            "--output-format",
            "json",
            "--print-timeout",
            "20s",
        ],
    };
    let bytes = run(executable, arguments, directory.path()).await?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).context("AGY report is not valid JSON")?;
    let name = match report {
        AgyReport::Models => "models",
        AgyReport::Usage => "usage",
    };
    ensure!(
        value["status"] == "SUCCESS" && value["command"]["name"] == name,
        "AGY did not return the requested read-only report; sign in with agy and retry"
    );
    ensure!(
        value["num_turns"].as_u64() == Some(0)
            && value["usage"]["total_tokens"].as_u64() == Some(0),
        "AGY report unexpectedly ran an agent turn"
    );
    value
        .pointer("/command/data")
        .cloned()
        .context("AGY report has no command data")
}

async fn run(executable: &Path, arguments: &[&str], directory: &Path) -> anyhow::Result<Vec<u8>> {
    let guard = directory.join("browser-disabled");
    let mut paths = vec![guard.clone()];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    let mut command = tokio::process::Command::from(super::launcher_command(executable));
    if directory.join(".gemini").is_dir() {
        // Force the same file credentials as inference, bypassing the OS keyring.
        command.env_clear();
        for key in [
            "PATH",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        super::set_tokio_home_env(&mut command, directory);
        command.env("SSH_TTY", "codex-mixin-scoped");
    }
    command
        .args(arguments)
        .current_dir(directory)
        .env("PATH", std::env::join_paths(paths)?)
        .env("BROWSER", guard.join("open"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    super::prepare_background_tokio_command(&mut command);
    super::isolate_tokio_process_group(&mut command);
    let child = command.spawn().context("start AGY read-only report")?;
    let mut running = ReportChild {
        pid: child.id(),
        child,
    };
    let stdout = running
        .child
        .stdout
        .take()
        .context("AGY stdout pipe is missing")?;
    let stderr = running
        .child
        .stderr
        .take()
        .context("AGY stderr pipe is missing")?;
    let outcome = tokio::time::timeout(REPORT_TIMEOUT, async {
        tokio::try_join!(
            read_pipe(stdout, MAX_REPORT_BYTES),
            read_pipe(stderr, MAX_DIAGNOSTIC_BYTES),
            async { running.child.wait().await.context("wait for AGY report") },
        )
    })
    .await;
    super::terminate_isolated_tokio_child(running.pid, &mut running.child).await;
    running.pid = None;
    let (stdout, _stderr, status) = outcome.context("AGY report timed out after 30 seconds")??;
    // Native diagnostics may contain account or credential details.
    ensure!(
        status.success(),
        "AGY report failed with {status}; run agy to check login and connectivity"
    );
    Ok(stdout)
}

async fn read_pipe(pipe: impl AsyncRead + Unpin, limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.take(limit as u64 + 1).read_to_end(&mut bytes).await?;
    ensure!(bytes.len() <= limit, "AGY report exceeded its output limit");
    Ok(bytes)
}

fn prepare_report_directory() -> anyhow::Result<tempfile::TempDir> {
    let directory = tempfile::tempdir().context("create isolated AGY report directory")?;
    let guard = directory.path().join("browser-disabled");
    std::fs::create_dir(&guard)?;
    // Headless usage must fail instead of opening an interactive login browser.
    #[cfg(unix)]
    for name in ["open", "xdg-open"] {
        let path = guard.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 1\n")?;
        super::make_private_executable(&path)?;
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn report_output_is_bounded() {
        assert!(read_pipe(&b"12345"[..], 4).await.is_err());
        assert_eq!(read_pipe(&b"1234"[..], 4).await.unwrap(), b"1234");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_use_inference_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("agy-fixture");
        let credentials = directory.path().join("source-token");
        std::fs::write(&credentials, "fixture-only").unwrap();
        std::fs::write(&program, r#"#!/bin/sh
if [ "$1" = --version ]; then echo 1.3.5; exit 0; fi
[ "$SSH_TTY" = codex-mixin-scoped ] || exit 2
[ "$(cat "$HOME/.gemini/antigravity-cli/antigravity-oauth-token")" = fixture-only ] || exit 3
printf '%s' '{"status":"SUCCESS","num_turns":0,"usage":{"total_tokens":0},"command":{"name":"models","data":{"models":[]}}}'
"#).unwrap();
        super::super::make_private_executable(&program).unwrap();
        assert!(
            read_report(&program, AgyReport::Models, Some(credentials.clone()))
                .await
                .is_ok()
        );
        assert_eq!(
            std::fs::read_to_string(credentials).unwrap(),
            "fixture-only"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn only_read_only_reports_pass() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("agy-fixture");
        for turns in [0, 1] {
            let payload = serde_json::json!({"status":"SUCCESS","num_turns":turns,
                "usage":{"total_tokens":0},"command":{"name":"models","data":{"models":[]}}});
            std::fs::write(&program, format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 1.3.5; exit 0; fi\ncat <<'REPORT'\n{payload}\nREPORT\n"
            )).unwrap();
            super::super::make_private_executable(&program).unwrap();
            assert_eq!(
                read_report(&program, AgyReport::Models, None).await.is_ok(),
                turns == 0
            );
        }
    }
}
