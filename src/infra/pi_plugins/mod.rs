use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, ensure};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::platform::{agent_cli, ensure_owner_only_dir, prepare_background_tokio_command};

pub(crate) use crate::provider::{
    PI_BRIDGE_PLACEHOLDER_URL, PI_BRIDGE_PROVIDER_ID, is_managed_pi_provider as is_managed_provider,
};
pub(crate) const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";
pub(crate) const PI_VERSION: &str = "1.6.0";
pub(crate) const NPM_REGISTRY: &str = "https://registry.npmmirror.com";
const READY_PREFIX: &str = "mixin-pi-ready ";

pub(crate) fn root() -> anyhow::Result<PathBuf> {
    let config = std::path::absolute(crate::config::stored_config_path())?;
    Ok(config
        .parent()
        .context("Mixin config has no parent")?
        .join("pi-runtime"))
}

pub(crate) fn pi_package_path(root: &Path) -> PathBuf {
    root.join("node_modules").join(PI_PACKAGE)
}

pub(crate) fn pi_cli_path(root: &Path) -> PathBuf {
    pi_package_path(root).join("dist/bundle/cli.js")
}

pub(crate) fn node() -> anyhow::Result<PathBuf> {
    agent_cli::find_command("node").context("Pi plugins require Node.js 22.19 or newer")
}

pub(crate) fn configure_pi_command(command: &mut Command, root: &Path) {
    command
        .current_dir(root.join("workspace"))
        .env("PI_CODING_AGENT_DIR", root.join("agent"))
        .env("PI_CODING_AGENT_SESSION_DIR", root.join("agent/sessions"))
        .env("PI_TELEMETRY", "0")
        .env("npm_config_registry", NPM_REGISTRY)
        .env("npm_config_cache", root.join("npm-cache"))
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .env_remove("PI_PACKAGE_DIR");
}

pub(crate) async fn invalidate(root: &Path) -> anyhow::Result<()> {
    let file = root.join("generation");
    let generation = crate::gateway_access::generate_gateway_api_key()?;
    tokio::task::spawn_blocking(move || {
        crate::platform::write_owner_only(&file, generation.as_bytes())
    })
    .await??;
    Ok(())
}

#[derive(Clone)]
pub(crate) struct BridgeEndpoint {
    pub(crate) base_url: String,
    pub(crate) token: String,
}

#[derive(Default)]
pub(crate) struct Bridge {
    running: Mutex<Option<RunningBridge>>,
}

/// One bridge per process: gateway reloads reuse it, plugin changes restart it via `invalidate`.
pub(crate) static SHARED_BRIDGE: LazyLock<Bridge> = LazyLock::new(Bridge::default);

struct RunningBridge {
    generation: Option<String>,
    endpoint: BridgeEndpoint,
    child: Child,
}

impl Bridge {
    pub(crate) async fn endpoint(&self) -> anyhow::Result<BridgeEndpoint> {
        let root = root()?;
        let mut running = self.running.lock().await;
        let generation = match tokio::fs::read_to_string(root.join("generation")).await {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("read Pi plugin generation"),
        };
        if let Some(running) = running.as_mut()
            && running.generation == generation
            && running.child.try_wait()?.is_none()
        {
            return Ok(running.endpoint.clone());
        }
        let next = RunningBridge::start(root, generation).await?;
        let endpoint = next.endpoint.clone();
        *running = Some(next);
        Ok(endpoint)
    }
}

impl RunningBridge {
    async fn start(root: PathBuf, generation: Option<String>) -> anyhow::Result<Self> {
        ensure!(
            pi_cli_path(&root).is_file(),
            "Pi plugin runtime is not initialized"
        );
        let bridge_script = root.join("bridge.mjs");
        let path = bridge_script.clone();
        tokio::task::spawn_blocking(move || {
            ensure_owner_only_dir(path.parent().context("Pi bridge path has no parent")?)?;
            crate::platform::write_owner_only(&path, include_bytes!("bridge.mjs"))
        })
        .await??;
        let token = crate::gateway_access::generate_gateway_api_key()?;
        let mut command = Command::new(node()?);
        configure_pi_command(&mut command, &root);
        prepare_background_tokio_command(&mut command);
        command
            .arg(bridge_script)
            .env("MIXIN_PI_BRIDGE_TOKEN", &token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().context("start isolated Pi runtime")?;
        let stdout = child.stdout.take().context("Pi bridge has no stdout")?;
        let mut stderr = child.stderr.take().context("Pi bridge has no stderr")?;
        let capturing_stderr = Arc::new(AtomicBool::new(true));
        let stderr_tail = Arc::new(StdMutex::new(Vec::<u8>::new()));
        let capturing = Arc::clone(&capturing_stderr);
        let tail = Arc::clone(&stderr_tail);
        let stderr_task = tokio::spawn(async move {
            const MAX_STDERR_BYTES: usize = 8 * 1024;
            let mut chunk = [0u8; 1024];
            while let Ok(read) = stderr.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                if !capturing.load(Ordering::Relaxed) {
                    continue;
                }
                if let Ok(mut buf) = tail.lock() {
                    let overflow = (buf.len() + read).saturating_sub(MAX_STDERR_BYTES);
                    if overflow > 0 {
                        let drain = overflow.min(buf.len());
                        buf.drain(..drain);
                    }
                    let start = read.saturating_sub(MAX_STDERR_BYTES);
                    buf.extend_from_slice(&chunk[start..read]);
                }
            }
        });
        let mut stdout = BufReader::new(stdout);
        let port = tokio::time::timeout(Duration::from_secs(30), async {
            let mut startup = (&mut stdout).take(128 * 1024);
            let mut line = String::new();
            while startup.read_line(&mut line).await? != 0 {
                if let Some(port) = line.trim_end().strip_prefix(READY_PREFIX) {
                    return port.parse::<u16>().context("invalid Pi bridge port");
                }
                line.clear();
            }
            anyhow::bail!("Pi bridge exited before it was ready")
        })
        .await
        .context("Pi bridge startup timed out");
        let port = match port.and_then(|result| result) {
            Ok(port) => {
                capturing_stderr.store(false, Ordering::Relaxed);
                port
            }
            Err(error) => {
                let _ = child.kill().await;
                let _ = tokio::time::timeout(Duration::from_millis(200), stderr_task).await;
                let detail = stderr_tail
                    .lock()
                    .ok()
                    .map(|buf| String::from_utf8_lossy(&buf).trim().to_owned())
                    .filter(|text| !text.is_empty());
                if let Some(detail) = detail {
                    return Err(error.context(format!("Pi bridge stderr: {detail}")));
                }
                return Err(error);
            }
        };
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await;
        });
        Ok(Self {
            generation,
            child,
            endpoint: BridgeEndpoint {
                base_url: format!("http://127.0.0.1:{port}"),
                token,
            },
        })
    }
}

impl Drop for RunningBridge {
    fn drop(&mut self) {
        // The private stdin pipe ties the bridge lifetime to its Rust owner.
        self.child.stdin.take();
    }
}
