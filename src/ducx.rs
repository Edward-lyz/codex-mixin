//! Managed DUCX authentication capture.
//!
//! DUCX is Baidu's Codex fork. Its default model proxy mints a per-session
//! `comate_custom_header` (and bearer token) from the login state. We override
//! its OneAPI base URL with a one-shot loopback endpoint, let DUCX perform its
//! real auth handshake, and capture the native headers from its warmup request.
//! Mixin then injects those headers into its own upstream request.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use reqwest::header::HeaderMap;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::auth_capture::{CaptureProxy, CaptureTrigger, REPORT_CLIENT_TOKEN_HEADER};

/// DUCX platform identity required for the comate source-auth handshake.
const DUCX_PLATFORM: &str = "AIIDE-terminal";
const DATA_REPORT_BASE_URL: &[u8] = b"http://ducc-data.baidu-int.com:8501";
const DATA_REPORT_STDERR_LIMIT: u64 = 8 * 1024;
/// Captured headers are reused until this TTL elapses to avoid spawning DUCX per
/// request. The minted token is short lived upstream, so keep the window small.
const HEADER_TTL: Duration = Duration::from_secs(600);

struct CapturedHeaders {
    headers: HeaderMap,
    at: Instant,
}

pub(crate) struct DucxRuntime {
    executable: PathBuf,
    home: PathBuf,
    cached: Mutex<Option<CapturedHeaders>>,
}

impl DucxRuntime {
    pub(crate) async fn spawn(executable: PathBuf) -> anyhow::Result<Self> {
        ensure!(
            executable.is_file(),
            "DUCX executable does not exist: {}",
            executable.display()
        );
        let home = managed_home(&executable)?;
        Ok(Self {
            executable,
            home,
            cached: Mutex::new(None),
        })
    }

    pub(crate) async fn native_headers(&self, timeout: Duration) -> anyhow::Result<HeaderMap> {
        let mut cached = self.cached.lock().await;
        if let Some(entry) = cached.as_ref()
            && entry.at.elapsed() < HEADER_TTL
        {
            return Ok(entry.headers.clone());
        }
        let headers = self.mint_headers(timeout).await?;
        *cached = Some(CapturedHeaders {
            headers: headers.clone(),
            at: Instant::now(),
        });
        Ok(headers)
    }

    pub(crate) async fn report_client_token(&self, timeout: Duration) -> anyhow::Result<String> {
        let data_report = self.data_report_path()?;
        ensure!(
            data_report.is_file(),
            "managed DUCX data-report is missing: {}",
            data_report.display()
        );
        let username = managed_username(&self.home)?;
        let proxy = CaptureProxy::start(CaptureTrigger::ReportClientToken).await?;
        let patch_source = data_report.clone();
        let patch_directory = self.home.join(".baidu-cx/tmp");
        let capture_addr = proxy.addr;
        let executable = tokio::task::spawn_blocking(move || {
            patched_data_report(&patch_source, &patch_directory, capture_addr)
        })
        .await
        .context("join isolated DUCX data-report preparation")??;
        resign_executable(executable.as_ref()).await?;
        let codex_home = self.home.join(".baidu-cx");
        let body = serde_json::to_vec(&json!({
            "session_id": "codex-mixin-report-warmup",
            "model": "mixin/report-warmup",
            "cwd": ".",
            "prompt": "codex-mixin report warmup"
        }))?;
        let mut child = Command::new(&executable)
            .arg("--user-prompt-submit")
            .env("HOME", &self.home)
            .env("CODEX_HOME", &codex_home)
            .env("DUCX_USERNAME", &username)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "start isolated managed DUCX data-report {}",
                    executable.to_path_buf().display()
                )
            })?;
        let stderr = child
            .stderr
            .take()
            .context("capture DUCX data-report stderr")?;
        let stderr_task = tokio::spawn(async move {
            let mut output = Vec::new();
            stderr
                .take(DATA_REPORT_STDERR_LIMIT)
                .read_to_end(&mut output)
                .await
                .context("read DUCX data-report stderr")?;
            anyhow::Ok(output)
        });
        child
            .stdin
            .take()
            .context("capture DUCX data-report stdin")?
            .write_all(&body)
            .await
            .context("write DUCX data-report input")?;
        let capture = proxy.capture(timeout);
        tokio::pin!(capture);
        let capture_result = tokio::select! {
            biased;
            captured = &mut capture => captured.context(
                "DUCX data-report did not emit a report client token before the capture proxy closed",
            ),
            status = child.wait() => {
                let status = status.context("wait for DUCX data-report warmup")?;
                let stderr = stderr_task
                    .await
                    .context("join DUCX data-report stderr task")??;
                let stderr = String::from_utf8_lossy(&stderr);
                anyhow::bail!(
                    "DUCX data-report exited with {status} before emitting its client token{}{}",
                    if stderr.trim().is_empty() { "" } else { ": " },
                    stderr.trim()
                );
            }
        };
        let captured = match capture_result {
            Ok(captured) => captured,
            Err(error) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let stderr = stderr_task
                    .await
                    .context("join DUCX data-report stderr task")??;
                executable
                    .close()
                    .context("remove isolated DUCX data-report")?;
                let stderr = String::from_utf8_lossy(&stderr);
                if stderr.trim().is_empty() {
                    return Err(error);
                }
                return Err(error.context(format!("DUCX data-report stderr: {}", stderr.trim())));
            }
        };
        let token = captured
            .get(REPORT_CLIENT_TOKEN_HEADER)
            .context("DUCX data-report warmup response is missing its client token")?
            .to_str()
            .context("DUCX data-report client token is not valid UTF-8")?
            .to_owned();
        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = stderr_task.await;
        executable
            .close()
            .context("remove isolated DUCX data-report")?;
        Ok(token)
    }

    fn data_report_path(&self) -> anyhow::Result<PathBuf> {
        let install = self
            .executable
            .parent()
            .context("DUCX executable has no bin directory")?
            .parent()
            .context("DUCX executable has no install directory")?;
        Ok(install.join("hooks/data-report"))
    }

    async fn mint_headers(&self, timeout: Duration) -> anyhow::Result<HeaderMap> {
        let proxy = CaptureProxy::start(CaptureTrigger::NativeHeader).await?;
        let base_url_override = format!(
            "model_providers.oneapi.base_url=\"http://{}/v1\"",
            proxy.addr
        );
        let codex_home = self.home.join(".baidu-cx");
        let mut child = Command::new(&self.executable)
            .args([
                "-c",
                &base_url_override,
                "--disable",
                "hooks",
                "--disable",
                "plugins",
                "exec",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "codex-mixin auth warmup, reply ok",
            ])
            .current_dir(&self.home)
            .env("HOME", &self.home)
            .env("CODEX_HOME", &codex_home)
            .env("BAIDU_CX_PLATFORM", DUCX_PLATFORM)
            .env("DISABLE_DUCX_CLI_UPDATE", "1")
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start managed DUCX {}", self.executable.display()))?;
        let captured = proxy.capture(timeout).await.context(
            "DUCX did not emit an authenticated request before the capture proxy closed",
        )?;
        let _ = child.kill().await;
        let _ = child.wait().await;
        Ok(captured)
    }
}

fn patched_data_report(
    source: &Path,
    temporary_directory: &Path,
    capture_addr: std::net::SocketAddr,
) -> anyhow::Result<tempfile::TempPath> {
    let mut binary = std::fs::read(source)
        .with_context(|| format!("read managed DUCX data-report {}", source.display()))?;
    let matches = memchr::memmem::find_iter(&binary, DATA_REPORT_BASE_URL).collect::<Vec<_>>();
    ensure!(
        matches.len() == 1,
        "managed DUCX data-report must contain exactly one supported report base URL; found {}",
        matches.len()
    );
    let mut loopback_url = format!("http://{capture_addr}/").into_bytes();
    ensure!(
        loopback_url.len() <= DATA_REPORT_BASE_URL.len(),
        "DUCX report capture URL is longer than the embedded report base URL"
    );
    loopback_url.resize(DATA_REPORT_BASE_URL.len(), b'x');
    let offset = matches[0];
    binary[offset..offset + DATA_REPORT_BASE_URL.len()].copy_from_slice(&loopback_url);

    std::fs::create_dir_all(temporary_directory).with_context(|| {
        format!(
            "create DUCX temporary directory {}",
            temporary_directory.display()
        )
    })?;
    std::fs::set_permissions(temporary_directory, std::fs::Permissions::from_mode(0o700))
        .with_context(|| {
            format!(
                "secure DUCX temporary directory {}",
                temporary_directory.display()
            )
        })?;
    let mut executable = tempfile::Builder::new()
        .prefix(".codex-mixin-data-report-")
        .tempfile_in(temporary_directory)
        .context("create isolated DUCX data-report executable")?;
    executable
        .write_all(&binary)
        .context("write isolated DUCX data-report executable")?;
    executable
        .flush()
        .context("flush isolated DUCX data-report executable")?;
    executable
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o700))
        .context("make isolated DUCX data-report executable")?;
    Ok(executable.into_temp_path())
}

#[cfg(target_os = "macos")]
async fn resign_executable(path: &Path) -> anyhow::Result<()> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(path)
        .output()
        .await
        .context("start codesign for isolated DUCX data-report")?;
    ensure!(
        output.status.success(),
        "codesign isolated DUCX data-report failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn resign_executable(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

fn managed_username(home: &Path) -> anyhow::Result<String> {
    let login_dir = home.join(".comate/login-user");
    let mut usernames = std::fs::read_dir(&login_dir)
        .with_context(|| format!("read DUCX login directory {}", login_dir.display()))?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
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

fn managed_home(executable: &Path) -> anyhow::Result<PathBuf> {
    let bin = executable
        .parent()
        .context("DUCX executable has no bin directory")?;
    let install = bin
        .parent()
        .context("DUCX executable has no install directory")?;
    let root = install
        .parent()
        .context("DUCX executable has no .baidu-cx directory")?;
    ensure!(
        install.file_name().and_then(|value| value.to_str()) == Some("baidu-cx")
            && root.file_name().and_then(|value| value.to_str()) == Some(".baidu-cx"),
        "DUCX executable must use the managed HOME/.baidu-cx/baidu-cx/bin layout"
    );
    Ok(root
        .parent()
        .context("DUCX executable has no managed HOME")?
        .to_owned())
}

/// Default managed DUCX executable location under the Mixin-managed home.
pub(crate) fn default_ducx_executable() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    [
        home.join(".codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx"),
        home.join(".codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/codex"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_managed_home_for_ducx_layout() {
        let executable = Path::new("/tmp/codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx");
        assert_eq!(
            managed_home(executable).unwrap(),
            PathBuf::from("/tmp/codex-mixin/ducx/home")
        );
    }

    #[test]
    fn native_header_is_captured_from_shared_proxy() {
        assert_eq!(crate::auth_capture::NATIVE_HEADER, "comate_custom_header");
    }

    #[test]
    fn patches_only_the_embedded_report_base_url() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("data-report");
        let mut fixture = b"prefix:".to_vec();
        fixture.extend_from_slice(DATA_REPORT_BASE_URL);
        fixture.extend_from_slice(b":suffix");
        std::fs::write(&source, &fixture).unwrap();

        let executable = patched_data_report(
            &source,
            directory.path(),
            "127.0.0.1:12345".parse().unwrap(),
        )
        .unwrap();
        let patched = std::fs::read(executable).unwrap();

        assert_eq!(patched.len(), fixture.len());
        assert!(
            !patched
                .windows(DATA_REPORT_BASE_URL.len())
                .any(|window| window == DATA_REPORT_BASE_URL)
        );
        assert!(
            patched
                .windows(DATA_REPORT_BASE_URL.len())
                .any(|window| window == b"http://127.0.0.1:12345/xxxxxxxxxxxx")
        );
    }

    #[test]
    fn rejects_unknown_data_report_layout() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("data-report");
        std::fs::write(&source, b"unsupported binary").unwrap();

        let error = patched_data_report(
            &source,
            directory.path(),
            "127.0.0.1:12345".parse().unwrap(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("found 0"));
    }

    #[tokio::test]
    async fn reports_data_report_early_exit_without_waiting_for_timeout() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let install = home.join(".baidu-cx/baidu-cx");
        let executable = install.join("bin/ducx");
        let data_report = install.join("hooks/data-report");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(data_report.parent().unwrap()).unwrap();
        std::fs::create_dir_all(home.join(".comate/login-user")).unwrap();
        std::fs::write(&executable, b"ducx fixture").unwrap();
        std::fs::write(home.join(".comate/login-user/test-user"), b"").unwrap();
        std::fs::write(
            &data_report,
            b"#!/bin/sh\n# http://ducc-data.baidu-int.com:8501\nexit 7\n",
        )
        .unwrap();
        std::fs::set_permissions(&data_report, std::fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = DucxRuntime::spawn(executable).await.unwrap();
        let started = Instant::now();

        let error = runtime
            .report_client_token(Duration::from_secs(5))
            .await
            .unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            error
                .to_string()
                .contains("before emitting its client token"),
            "{error:#}"
        );
    }

    #[tokio::test]
    #[ignore = "requires a managed signed-in DUCX install"]
    async fn captures_real_data_report_token_without_uploading() {
        let executable = default_ducx_executable().unwrap();
        let runtime = DucxRuntime::spawn(executable).await.unwrap();

        let token = runtime
            .report_client_token(Duration::from_secs(5))
            .await
            .unwrap();

        assert!(!token.is_empty());
    }
}
