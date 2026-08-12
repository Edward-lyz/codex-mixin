//! Managed DUCX authentication capture.
//!
//! DUCX is Baidu's Codex fork. Its default model proxy mints a per-session
//! `comate_custom_header` (and bearer token) from the login state and sends the
//! OneAPI request in plaintext HTTP. We run a one-shot local forward proxy, let
//! DUCX perform its real auth handshake, and sniff the native headers off the
//! first proxied request that carries `comate_custom_header`. Mixin then injects
//! those headers into its own upstream request.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use reqwest::header::HeaderMap;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::auth_capture::CaptureProxy;

/// Auth handshake hosts DUCX must reach directly. Only the OneAPI inference host
/// is routed through our capture proxy; proxying the source-auth handshake makes
/// DUCX's `generate source auth` call fail, so these bypass the proxy.
const DIRECT_HOSTS: &str = "baidu-int.com,bcebos.com,baidu.com,openai.com,chatgpt.com";
/// DUCX platform identity required for the comate source-auth handshake.
const DUCX_PLATFORM: &str = "AIIDE-terminal";
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

    async fn mint_headers(&self, timeout: Duration) -> anyhow::Result<HeaderMap> {
        let proxy = CaptureProxy::start().await?;
        let proxy_url = format!("http://{}", proxy.addr);
        let codex_home = self.home.join(".baidu-cx");
        let mut child = Command::new(&self.executable)
            .args([
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
            .env("HTTP_PROXY", &proxy_url)
            .env("http_proxy", &proxy_url)
            .env("NO_PROXY", DIRECT_HOSTS)
            .env("no_proxy", DIRECT_HOSTS)
            .env("BAIDU_CX_PLATFORM", DUCX_PLATFORM)
            .env("DISABLE_DUCX_CLI_UPDATE", "1")
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start managed DUCX {}", self.executable.display()))?;
        let captured = tokio::time::timeout(timeout, proxy.captured)
            .await
            .context("DUCX did not emit an authenticated request in time")?
            .context("DUCX capture proxy closed before capturing native headers")?;
        let _ = child.start_kill();
        Ok(captured)
    }
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
}
