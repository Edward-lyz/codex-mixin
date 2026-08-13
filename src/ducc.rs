//! Managed DUCC authentication capture.
//!
//! DUCC is Baidu's Claude Code fork. Run one short `--print` turn through a
//! loopback HTTP proxy, capture its bearer authorization, then stop before the
//! warmup request reaches OneAPI.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use reqwest::header::HeaderMap;
use serde_json::json;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::auth_capture::{CaptureProxy, CaptureTrigger};

/// Auth handshake hosts DUCC must reach directly. Only the OneAPI inference host
/// is routed through our capture proxy.
const DIRECT_HOSTS: &str = "baidu-int.com,bcebos.com,baidu.com,openai.com,chatgpt.com";
const DUCC_PLATFORM: &str = "AIIDE-terminal";
const DUCC_AUTH_CARRIER_MODEL: &str = "GLM-5.2";
const HEADER_TTL: Duration = Duration::from_secs(600);

struct CapturedHeaders {
    headers: HeaderMap,
    at: Instant,
}

pub(crate) struct DuccRuntime {
    executable: PathBuf,
    home: PathBuf,
    api_key: String,
    cached: Mutex<Option<CapturedHeaders>>,
}

impl DuccRuntime {
    pub(crate) async fn spawn(executable: PathBuf, api_key: String) -> anyhow::Result<Self> {
        ensure!(
            executable.is_file(),
            "DUCC executable does not exist: {}",
            executable.display()
        );
        let home = managed_home(&executable)?;
        Ok(Self {
            executable,
            home,
            api_key,
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
        let proxy = CaptureProxy::start(CaptureTrigger::Authorization).await?;
        let proxy_url = format!("http://{}", proxy.addr);
        let settings = serde_json::to_string(&json!({
            "env": {
                "ANTHROPIC_API_KEY": self.api_key
            }
        }))
        .context("serialize DUCC warmup settings")?;
        let mut child = Command::new(&self.executable)
            .args([
                "--bare",
                "--app-source=one-api-token",
                "--no-ducc-system-prompt",
                "--disable-slash-commands",
                "--no-session-persistence",
                "--permission-mode",
                "dontAsk",
                "--prompt-suggestions",
                "false",
                "--tools",
                "",
                "--model",
                DUCC_AUTH_CARRIER_MODEL,
                "--settings",
                &settings,
                "--print",
                "--input-format",
                "text",
                "--output-format",
                "stream-json",
                "--verbose",
                "codex-mixin auth warmup, reply ok",
            ])
            .current_dir(&self.home)
            .env("HOME", &self.home)
            .env("HTTP_PROXY", &proxy_url)
            .env("http_proxy", &proxy_url)
            .env("HTTPS_PROXY", &proxy_url)
            .env("https_proxy", &proxy_url)
            .env("NO_PROXY", DIRECT_HOSTS)
            .env("no_proxy", DIRECT_HOSTS)
            .env("BAIDU_CC_PLATFORM", DUCC_PLATFORM)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .env("DISABLE_DUCC_CLI_UPDATE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start managed DUCC {}", self.executable.display()))?;
        let captured = proxy.capture(timeout).await.context(
            "DUCC did not emit an authenticated request before the capture proxy closed",
        )?;
        let _ = child.kill().await;
        let _ = child.wait().await;
        Ok(captured)
    }
}

fn managed_home(executable: &Path) -> anyhow::Result<PathBuf> {
    let bin = executable
        .parent()
        .context("DUCC executable has no bin directory")?;
    let install = bin
        .parent()
        .context("DUCC executable has no install directory")?;
    let root = install
        .parent()
        .context("DUCC executable has no .baidu-cc directory")?;
    ensure!(
        install.file_name().and_then(|value| value.to_str()) == Some("baidu-cc")
            && root.file_name().and_then(|value| value.to_str()) == Some(".baidu-cc"),
        "DUCC executable must use the managed HOME/.baidu-cc/baidu-cc/bin layout"
    );
    Ok(root
        .parent()
        .context("DUCC executable has no managed HOME")?
        .to_owned())
}

/// Default managed DUCC executable location under the Mixin-managed home.
pub(crate) fn default_ducc_executable() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    [
        home.join(".codex-mixin/ducc/home/.baidu-cc/baidu-cc/bin/ducc"),
        home.join(".codex-mixin/ducc/home/.baidu-cc/baidu-cc/bin/claude"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_managed_home_for_ducc_layout() {
        let executable = Path::new("/tmp/codex-mixin/ducc/home/.baidu-cc/baidu-cc/bin/ducc");
        assert_eq!(
            managed_home(executable).unwrap(),
            PathBuf::from("/tmp/codex-mixin/ducc/home")
        );
    }
}
