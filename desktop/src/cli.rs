//! Runs the real `codex-mixin` CLI. The desktop shell never reads or writes
//! CLI state itself: every query and change goes through `--no-tui` commands
//! whose JSON output and structured errors form the contract.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::Value;

const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const QUERY_TIMEOUT: Duration = Duration::from_secs(90);
const CHANGE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Daemons such as `service start` inherit the pipes; once the CLI exits its
/// own output is complete after this drain window.
const PIPE_DRAIN: Duration = Duration::from_secs(3);
const PROGRESS_PREFIX: &str = "MIXIN_PROGRESS ";
/// Tokio otherwise starts one worker per logical CPU for every short query.
const QUERY_WORKER_THREADS: &str = "2";

#[cfg(windows)]
const CLI_NAME: &str = "codex-mixin.exe";
#[cfg(not(windows))]
const CLI_NAME: &str = "codex-mixin";

/// A failed CLI call. `committed` mirrors the CLI error envelope: `Some(true)`
/// means the configuration was saved before the failure, `None` means unknown.
#[derive(Clone, Debug)]
pub struct CliError {
    pub message: String,
    pub committed: Option<bool>,
}

impl CliError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            committed: None,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

pub type CliResult<T> = Result<T, CliError>;

/// Receives `MIXIN_PROGRESS` lines while a change runs.
pub type Progress = Arc<dyn Fn(String) + Send + Sync>;

/// A secret passed through the child environment and referenced on the
/// command line as `@env:NAME`, so it never enters argv or logs.
#[derive(Clone)]
pub struct Secret {
    pub variable: &'static str,
    pub value: String,
}

pub struct Cli {
    executable: PathBuf,
    /// Changes run one at a time; read-only queries never wait behind them.
    change_gate: Mutex<()>,
}

impl Cli {
    /// Trusts only the packaged binary beside the app or a repository build,
    /// never `PATH` or the current directory.
    pub fn locate() -> anyhow::Result<Arc<Self>> {
        let current = std::env::current_exe()?;
        let directory = current
            .parent()
            .ok_or_else(|| anyhow::anyhow!("the desktop executable has no parent directory"))?;
        let packaged = directory.join(CLI_NAME);
        if packaged.is_file() {
            return Ok(Arc::new(Self::new(packaged)));
        }
        for ancestor in directory.ancestors() {
            if !ancestor.join("Cargo.toml").is_file() || !ancestor.join("src/main.rs").is_file() {
                continue;
            }
            // Development builds use whichever profile was built last.
            let newest = ["release", "debug"]
                .iter()
                .map(|profile| ancestor.join("target").join(profile).join(CLI_NAME))
                .filter_map(|candidate| {
                    let modified = candidate.metadata().and_then(|meta| meta.modified()).ok()?;
                    Some((modified, candidate))
                })
                .max_by_key(|(modified, _)| *modified);
            if let Some((_, candidate)) = newest {
                return Ok(Arc::new(Self::new(candidate)));
            }
        }
        anyhow::bail!("{}", rust_i18n::t!("cli.missing"))
    }

    fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            change_gate: Mutex::new(()),
        }
    }

    /// Runs a read-only command and decodes its JSON output.
    pub fn query<T: DeserializeOwned>(&self, args: &[&str]) -> CliResult<T> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let output = self.execute(&args, &[], QUERY_TIMEOUT, true, None)?;
        serde_json::from_str(&output).map_err(|error| {
            CliError::new(rust_i18n::t!(
                "cli.invalid_json",
                command = args.first().map_or("", String::as_str),
                error = error.to_string()
            ))
        })
    }

    /// Runs a configuration change. With `apply`, a successful change is
    /// followed by `config apply`; an apply failure reports the change as saved.
    pub fn change(
        &self,
        args: &[String],
        secrets: &[Secret],
        apply: bool,
        _repair_startup_on_denied: bool,
        progress: Option<Progress>,
    ) -> CliResult<String> {
        let _gate = self
            .change_gate
            .lock()
            .map_err(|_| CliError::new(rust_i18n::t!("cli.gate_poisoned")))?;
        let result = self.execute(args, secrets, CHANGE_TIMEOUT, false, progress.clone());
        #[cfg(windows)]
        let result = match result {
            Err(error)
                if _repair_startup_on_denied
                    && args == ["service", "autostart", "enable", "--json"]
                    && startup_task_access_denied(&error.message) =>
            {
                self.repair_startup_task(progress.clone())
            }
            other => other,
        };
        // A failure after the configuration was committed still applies it,
        // so the gateway serves what the CLI saved.
        let saved = match &result {
            Ok(_) => true,
            Err(error) => error.committed == Some(true),
        };
        if !apply || !saved {
            return result;
        }
        let apply_args = ["config".to_owned(), "apply".to_owned()];
        let applied = self.execute(&apply_args, &[], CHANGE_TIMEOUT, false, progress);
        match (result, applied) {
            (Ok(output), Ok(applied)) => Ok(format!("{output}\n{applied}").trim().to_owned()),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(CliError {
                message: rust_i18n::t!("cli.apply_failed", error = error.message).into(),
                committed: Some(true),
            }),
        }
    }

    #[cfg(windows)]
    fn repair_startup_task(&self, progress: Option<Progress>) -> CliResult<String> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        const REPAIR_SCRIPT: &str = include_str!("../windows/restore-gateway.ps1");
        let system_root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let Some(powershell) = system_root
            .map(|root| root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
            .filter(|powershell| powershell.is_file())
        else {
            return Err(CliError::new(rust_i18n::t!(
                "cli.startup_repair_unavailable"
            )));
        };
        if let Some(progress) = &progress {
            progress(rust_i18n::t!("cli.startup_repair_authorization").into_owned());
        }
        // The repair code comes from the app build. Only the paired CLI's
        // path is passed separately, without interpolating it into scripts.
        let command_text = format!(
            "& {{\n{REPAIR_SCRIPT}\n}} -CliPath $env:CODEX_MIXIN_REPAIR_CLI -RestoreAutostart -RepairLegacyTask"
        );
        let encoded = STANDARD.encode(
            command_text
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let mut command = Command::new(powershell);
        command
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(encoded)
            .env("CODEX_MIXIN_REPAIR_CLI", &self.executable);
        capture(&mut command, &[], CHANGE_TIMEOUT, false, progress).map_err(|error| {
            CliError::new(rust_i18n::t!(
                "cli.startup_repair_failed",
                error = error.message
            ))
        })
    }

    fn execute(
        &self,
        args: &[String],
        secrets: &[Secret],
        timeout: Duration,
        read_only: bool,
        progress: Option<Progress>,
    ) -> CliResult<String> {
        let mut command = Command::new(&self.executable);
        command.arg("--no-tui").arg("--json-errors").args(args);
        #[cfg(target_os = "macos")]
        {
            // Finder launches with a system-only PATH. Package-manager scripts
            // can be found by absolute path but still need their interpreters.
            let inherited = std::env::var_os("PATH");
            let paths = inherited
                .iter()
                .flat_map(std::env::split_paths)
                .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
            let path =
                std::env::join_paths(paths).map_err(|error| CliError::new(error.to_string()))?;
            command.env("PATH", path);
        }
        if let Some(directory) = self.executable.parent() {
            command.current_dir(directory);
        }
        if read_only && std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
            command.env("TOKIO_WORKER_THREADS", QUERY_WORKER_THREADS);
        }
        for secret in secrets {
            command.env(secret.variable, &secret.value);
        }
        capture(&mut command, secrets, timeout, read_only, progress)
    }
}

pub(crate) fn startup_task_access_denied(message: &str) -> bool {
    message.contains("0x80070005")
        && ["register", "start", "stop"].iter().any(|operation| {
            message.contains(&format!("{operation} gateway scheduled task failed"))
        })
}

fn capture(
    command: &mut Command,
    secrets: &[Secret],
    timeout: Duration,
    read_only: bool,
    progress: Option<Progress>,
) -> CliResult<String> {
    // GUI callers never hand a terminal to the CLI; DUCX relies on this
    // to open its own visible QR login window.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console_window(command);
    let mut child = command.spawn().map_err(|error| {
        CliError::new(rust_i18n::t!("cli.spawn_failed", error = error.to_string()))
    })?;
    let stdout = PipeReader::spawn(child.stdout.take(), None, Vec::new());
    let stderr = PipeReader::spawn(child.stderr.take(), progress, secrets.to_vec());
    let status = wait_with_deadline(&mut child, timeout, read_only)?;
    let output = redact(&stdout.collect()?, secrets);
    let errors = redact(&stderr.collect()?, secrets);
    if status.success() {
        return Ok(output);
    }
    Err(decode_error(&errors, &output, status.code()))
}

fn wait_with_deadline(
    child: &mut Child,
    timeout: Duration,
    read_only: bool,
) -> CliResult<ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let key = if read_only {
                    "cli.query_timeout"
                } else {
                    "cli.change_timeout"
                };
                return Err(CliError::new(rust_i18n::t!(key)));
            }
            Err(error) => return Err(CliError::new(error.to_string())),
        }
    }
}

/// Collects one pipe on its own thread so stdout and stderr never block each
/// other. Progress lines on stderr are redacted and forwarded as they arrive.
struct PipeReader {
    buffer: Arc<Mutex<PipeBuffer>>,
    done: mpsc::Receiver<()>,
}

#[derive(Default)]
struct PipeBuffer {
    bytes: Vec<u8>,
    overflowed: bool,
    error: Option<String>,
}

impl PipeReader {
    fn spawn<R: Read + Send + 'static>(
        pipe: Option<R>,
        progress: Option<Progress>,
        secrets: Vec<Secret>,
    ) -> Self {
        let buffer = Arc::new(Mutex::new(PipeBuffer::default()));
        let (sender, done) = mpsc::channel();
        let Some(mut pipe) = pipe else {
            let _ = sender.send(());
            return Self { buffer, done };
        };
        let shared = buffer.clone();
        thread::spawn(move || {
            let mut line = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let count = match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(error) => {
                        if let Ok(mut state) = shared.lock() {
                            state.error = Some(error.to_string());
                        }
                        break;
                    }
                };
                if let Ok(mut state) = shared.lock() {
                    // Past the limit the pipe keeps draining so the child
                    // never blocks on a full pipe; the excess is dropped.
                    if state.bytes.len() + count > OUTPUT_LIMIT {
                        state.overflowed = true;
                    } else {
                        state.bytes.extend_from_slice(&chunk[..count]);
                    }
                }
                let Some(progress) = &progress else {
                    continue;
                };
                for byte in &chunk[..count] {
                    if *byte != b'\n' {
                        line.push(*byte);
                        continue;
                    }
                    let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                    line.clear();
                    if let Some(message) = text.strip_prefix(PROGRESS_PREFIX) {
                        progress(redact(message, &secrets));
                    }
                }
            }
            let _ = sender.send(());
        });
        Self { buffer, done }
    }

    /// Returns what the pipe produced. A daemon may keep the pipe open after
    /// the CLI exits; after the drain window the bytes read so far are the
    /// CLI's complete output.
    fn collect(self) -> CliResult<String> {
        let _ = self.done.recv_timeout(PIPE_DRAIN);
        let state = self
            .buffer
            .lock()
            .map_err(|_| CliError::new(rust_i18n::t!("cli.worker_stopped")))?;
        if state.overflowed {
            return Err(CliError::new(rust_i18n::t!("cli.output_limit")));
        }
        if let Some(error) = &state.error {
            return Err(CliError::new(error.clone()));
        }
        Ok(String::from_utf8_lossy(&state.bytes).into_owned())
    }
}

fn redact(value: &str, secrets: &[Secret]) -> String {
    let mut value = value.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.value.is_empty()) {
        let encoded = serde_json::to_string(&secret.value).unwrap_or_default();
        let encoded = encoded.trim_matches('"');
        value = value
            .replace(encoded, "<redacted>")
            .replace(&secret.value, "<redacted>");
    }
    value
}

fn decode_error(errors: &str, output: &str, code: Option<i32>) -> CliError {
    for line in errors.lines().rev() {
        let Ok(envelope) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(error) = envelope.get("error") else {
            continue;
        };
        return CliError {
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            committed: error.get("committed").and_then(Value::as_bool),
        };
    }
    let detail = if errors.trim().is_empty() {
        output
    } else {
        errors
    };
    CliError::new(rust_i18n::t!(
        "cli.exit_code",
        code = code.map_or_else(|| "?".to_owned(), |code| code.to_string()),
        detail = detail.trim()
    ))
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_: &mut Command) {}
