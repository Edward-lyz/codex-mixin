//! Managed DUCX authentication capture.
//!
//! DUCX is Baidu's Codex fork. Its default model proxy mints a per-session
//! `comate_custom_header` (and bearer token) from the login state. We ask the
//! DUCX wrapper to launch Mixin's internal carrier without creating a model
//! turn, then reconstruct the native headers from its versioned pipe output.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use super::capture::{CaptureProxy, REPORT_CLIENT_TOKEN_HEADER};
use crate::ducx_auth_carrier::{DucxAuthCarrier, PREFIX, VERSION};

/// DUCX platform identity required for the comate source-auth handshake.
const DUCX_PLATFORM: &str = "AIIDE-terminal";
const DATA_REPORT_BASE_URL: &[u8] = b"http://ducc-data.baidu-int.com:8501";
const DATA_REPORT_STDERR_LIMIT: u64 = 8 * 1024;
const AUTH_CARRIER_STDOUT_LIMIT: u64 = 8 * 1024;
const NATIVE_HEADER: &str = "comate_custom_header";
/// Captured headers are refreshed on demand after one minute. No background
/// process runs while the gateway is idle.
const HEADER_TTL: Duration = Duration::from_secs(60);

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

    pub(crate) async fn invalidate_headers(&self) {
        *self.cached.lock().await = None;
    }

    pub(crate) async fn report_client_token(&self, timeout: Duration) -> anyhow::Result<String> {
        let data_report = self.data_report_path()?;
        ensure!(
            data_report.is_file(),
            "managed DUCX data-report is missing: {}",
            data_report.display()
        );
        let username = managed_username(&self.home)?;
        let proxy = CaptureProxy::start().await?;
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
        let mut command = Command::new(&executable);
        crate::platform::isolate_tokio_process_group(&mut command);
        crate::platform::prepare_background_tokio_command(&mut command);
        crate::platform::set_tokio_home_env(&mut command, &self.home);
        let mut child = command
            .arg("--user-prompt-submit")
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
        let process_group_id = child.id();
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
        let write_result = child
            .stdin
            .take()
            .context("capture DUCX data-report stdin")?
            .write_all(&body)
            .await;
        if let Err(error) = write_result
            && error.kind() != std::io::ErrorKind::BrokenPipe
        {
            return Err(error).context("write DUCX data-report input");
        }
        let capture = proxy.capture(timeout);
        tokio::pin!(capture);
        let capture_result = tokio::select! {
            biased;
            captured = &mut capture => captured.context(
                "DUCX data-report did not emit a report client token before the capture proxy closed",
            ),
            status = child.wait() => {
                let status = status.context("wait for DUCX data-report warmup")?;
                crate::platform::terminate_isolated_tokio_child(process_group_id, &mut child).await;
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
                crate::platform::terminate_isolated_tokio_child(process_group_id, &mut child).await;
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
        crate::platform::terminate_isolated_tokio_child(process_group_id, &mut child).await;
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
        Ok(install.join(data_report_relative_path()))
    }

    async fn mint_headers(&self, timeout: Duration) -> anyhow::Result<HeaderMap> {
        let carrier_executable = std::env::current_exe().context("resolve DUCX auth carrier")?;
        let codex_home = self.home.join(".baidu-cx");
        let mut command = Command::new(&self.executable);
        crate::platform::isolate_tokio_process_group(&mut command);
        // Keep the DUCX header-capture child from popping a console window on
        // every request that needs Baidu auth.
        crate::platform::prepare_background_tokio_command(&mut command);
        crate::platform::set_tokio_home_env(&mut command, &self.home);
        let mut child = command
            .args(["--disable", "hooks", "--disable", "plugins", "sandbox"])
            .arg(carrier_executable)
            .arg("ducx-auth-carrier")
            .current_dir(&self.home)
            .env("CODEX_HOME", &codex_home)
            .env("BAIDU_CX_PLATFORM", DUCX_PLATFORM)
            .env("DISABLE_DUCX_CLI_UPDATE", "1")
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start managed DUCX {}", self.executable.display()))?;
        let process_group_id = child.id();
        let stdout = child
            .stdout
            .take()
            .context("capture DUCX auth carrier output")?;
        let capture_result = capture_auth_carrier(stdout, timeout).await;
        crate::platform::terminate_isolated_tokio_child(process_group_id, &mut child).await;
        capture_result
    }
}

async fn capture_auth_carrier(
    stdout: tokio::process::ChildStdout,
    timeout: Duration,
) -> anyhow::Result<HeaderMap> {
    let capture = async {
        let mut lines = BufReader::new(stdout.take(AUTH_CARRIER_STDOUT_LIMIT)).lines();
        while let Some(line) = lines.next_line().await? {
            let Some(payload) = line.strip_prefix(PREFIX) else {
                continue;
            };
            let output: DucxAuthCarrier =
                serde_json::from_str(payload).context("parse DUCX auth carrier output")?;
            ensure!(
                output.version == VERSION,
                "unsupported DUCX auth carrier version {}",
                output.version
            );
            ensure!(
                !output.model_token.is_empty() && !output.custom_header.is_empty(),
                "DUCX auth carrier returned empty credentials"
            );
            let mut headers = HeaderMap::new();
            let authorization = HeaderValue::from_str(&format!("Bearer {}", output.model_token))
                .context("DUCX model token is not a valid HTTP header value")?;
            let custom_header = HeaderValue::from_str(&output.custom_header)
                .context("DUCX custom header is not a valid HTTP header value")?;
            headers.insert(AUTHORIZATION, authorization);
            headers.insert(HeaderName::from_static(NATIVE_HEADER), custom_header);
            return Ok(headers);
        }
        anyhow::bail!("DUCX auth carrier exited without credentials")
    };
    tokio::time::timeout(timeout, capture)
        .await
        .context("DUCX auth carrier did not return credentials in time")?
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
    crate::platform::set_owner_only_dir_mode(temporary_directory).with_context(|| {
        format!(
            "secure DUCX temporary directory {}",
            temporary_directory.display()
        )
    })?;
    let mut builder = tempfile::Builder::new();
    builder.prefix(".codex-mixin-data-report-");
    builder.suffix(crate::platform::EXECUTABLE_SUFFIX);
    let mut executable = builder
        .tempfile_in(temporary_directory)
        .context("create isolated DUCX data-report executable")?;
    executable
        .write_all(&binary)
        .context("write isolated DUCX data-report executable")?;
    executable
        .flush()
        .context("flush isolated DUCX data-report executable")?;
    crate::platform::make_private_executable_on(executable.as_file())
        .context("make isolated DUCX data-report executable")?;
    Ok(executable.into_temp_path())
}

async fn resign_executable(path: &Path) -> anyhow::Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || crate::platform::prepare_modified_executable(&path))
        .await
        .context("join isolated DUCX data-report signing")?
        .context("sign isolated DUCX data-report")
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

/// Path of the data-report hook relative to a DUCX install directory.
pub(crate) fn data_report_relative_path() -> PathBuf {
    Path::new("hooks").join(crate::platform::executable_file_name("data-report"))
}

/// Launcher names inside a DUCX package `bin` directory, preferred first.
/// `ducx` is the managed entry point; packages ship the same launcher as
/// `baidu-codex` and `codex`.
pub(crate) const DUCX_LAUNCHER_STEMS: [&str; 3] = ["ducx", "baidu-codex", "codex"];

/// Default managed DUCX executable location under the Mixin-managed home.
pub(crate) fn default_ducx_executable() -> Option<PathBuf> {
    let bin = crate::platform::home_dir_required()
        .ok()?
        .join(".codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin");
    DUCX_LAUNCHER_STEMS
        .into_iter()
        .map(|stem| bin.join(crate::platform::executable_file_name(stem)))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[cfg(unix)]
    fn process_is_running(pid: &str) -> bool {
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        output.status.success()
            && state
                .split_whitespace()
                .next()
                .is_some_and(|state| !state.starts_with('Z'))
    }

    #[test]
    fn derives_managed_home_for_ducx_layout() {
        let executable = Path::new("/tmp/codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx");
        assert_eq!(
            managed_home(executable).unwrap(),
            PathBuf::from("/tmp/codex-mixin/ducx/home")
        );
    }

    #[test]
    fn native_header_name_matches_ducx_config() {
        assert_eq!(NATIVE_HEADER, "comate_custom_header");
    }

    #[tokio::test]
    async fn invalidates_cached_native_headers() {
        let runtime = DucxRuntime {
            executable: PathBuf::from("/tmp/ducx"),
            home: PathBuf::from("/tmp"),
            cached: Mutex::new(Some(CapturedHeaders {
                headers: HeaderMap::new(),
                at: Instant::now(),
            })),
        };

        runtime.invalidate_headers().await;

        assert!(runtime.cached.lock().await.is_none());
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[tokio::test]
    async fn native_header_capture_avoids_reportable_turn() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let executable = home.join(".baidu-cx/baidu-cx/bin/ducx");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(
            &executable,
            r#"#!/bin/sh
/bin/sleep 30 &
descendant=$!
printf '%s' "$descendant" > "$HOME/descendant.pid"
uses_sandbox=false
uses_carrier=false
for argument in "$@"; do
  case "$argument" in
    sandbox)
      uses_sandbox=true
      ;;
    ducx-auth-carrier)
      uses_carrier=true
      ;;
    exec)
      printf 'reported\n' >> "$HOME/report-count"
      ;;
  esac
done
[ "$uses_sandbox" = true ] && [ "$uses_carrier" = true ] || exit 7
printf '%s\n' 'CODEX_MIXIN_DUCX_AUTH_V1={"version":1,"model_token":"model-token","custom_header":"fixture"}'
wait
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = DucxRuntime::spawn(executable).await.unwrap();

        let headers = runtime
            .native_headers(Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(headers[NATIVE_HEADER], "fixture");
        assert_eq!(headers[AUTHORIZATION], "Bearer model-token");
        assert!(
            !home.join("report-count").exists(),
            "DUCX header capture created a reportable turn"
        );
        let descendant_pid = std::fs::read_to_string(home.join("descendant.pid")).unwrap();
        let descendant_pid = descendant_pid.trim();
        let descendant_alive = process_is_running(descendant_pid);
        if descendant_alive
            && let Ok(process_id) = descendant_pid.parse::<i32>()
            && let Some(process_id) = rustix::process::Pid::from_raw(process_id)
        {
            let _ = rustix::process::kill_process(process_id, rustix::process::Signal::KILL);
        }

        assert!(!descendant_alive, "DUCX warmup descendant remained alive");
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
