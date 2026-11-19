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
            for profile in ["release", "debug"] {
                let candidate = ancestor.join("target").join(profile).join(CLI_NAME);
                if candidate.is_file() {
                    return Ok(Arc::new(Self::new(candidate)));
                }
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
        progress: Option<Progress>,
    ) -> CliResult<String> {
        let _gate = self
            .change_gate
            .lock()
            .map_err(|_| CliError::new(rust_i18n::t!("cli.gate_poisoned")))?;
        let output = self.execute(args, secrets, CHANGE_TIMEOUT, false, progress.clone())?;
        if !apply {
            return Ok(output);
        }
        let apply_args = ["config".to_owned(), "apply".to_owned()];
        match self.execute(&apply_args, &[], CHANGE_TIMEOUT, false, progress) {
            Ok(applied) => Ok(format!("{output}\n{applied}").trim().to_owned()),
            Err(error) => Err(CliError {
                message: rust_i18n::t!("cli.apply_failed", error = error.message).into(),
                committed: Some(true),
            }),
        }
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
        command
            .arg("--no-tui")
            .arg("--json-errors")
            .args(args)
            // GUI callers never hand a terminal to the CLI; DUCX relies on
            // this to open its own visible QR login window.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = self.executable.parent() {
            command.current_dir(directory);
        }
        if read_only && std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
            command.env("TOKIO_WORKER_THREADS", QUERY_WORKER_THREADS);
        }
        for secret in secrets {
            command.env(secret.variable, &secret.value);
        }
        hide_console_window(&mut command);
        let mut child = command.spawn().map_err(|error| {
            CliError::new(rust_i18n::t!("cli.spawn_failed", error = error.to_string()))
        })?;
        let stdout = PipeReader::spawn(child.stdout.take(), None);
        let stderr = PipeReader::spawn(child.stderr.take(), progress);
        let status = wait_with_deadline(&mut child, timeout, read_only)?;
        let output = redact(&stdout.collect()?, secrets);
        let errors = redact(&stderr.collect()?, secrets);
        if status.success() {
            return Ok(output);
        }
        Err(decode_error(&errors, &output, status.code()))
    }
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
/// other. Progress lines on stderr are forwarded as they arrive.
struct PipeReader {
    receiver: mpsc::Receiver<CliResult<String>>,
}

impl PipeReader {
    fn spawn<R: Read + Send + 'static>(pipe: Option<R>, progress: Option<Progress>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let Some(mut pipe) = pipe else {
            let _ = sender.send(Ok(String::new()));
            return Self { receiver };
        };
        thread::spawn(move || {
            let mut output = Vec::new();
            let mut line = Vec::new();
            let mut buffer = [0_u8; 4096];
            let result = loop {
                let count = match pipe.read(&mut buffer) {
                    Ok(0) => break Ok(String::from_utf8_lossy(&output).into_owned()),
                    Ok(count) => count,
                    Err(error) => break Err(CliError::new(error.to_string())),
                };
                if output.len() + count > OUTPUT_LIMIT {
                    break Err(CliError::new(rust_i18n::t!("cli.output_limit")));
                }
                output.extend_from_slice(&buffer[..count]);
                let Some(progress) = &progress else {
                    continue;
                };
                for byte in &buffer[..count] {
                    if *byte != b'\n' {
                        line.push(*byte);
                        continue;
                    }
                    let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                    line.clear();
                    if let Some(message) = text.strip_prefix(PROGRESS_PREFIX) {
                        progress(message.to_owned());
                    }
                }
            };
            let _ = sender.send(result);
        });
        Self { receiver }
    }

    fn collect(self) -> CliResult<String> {
        match self.receiver.recv_timeout(PIPE_DRAIN) {
            Ok(result) => result,
            // A daemon still holds the pipe; the CLI's own output has ended.
            Err(_) => Ok(String::new()),
        }
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
