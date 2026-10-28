//! Runs the real CLI as a child process. The panel never reaches CLI
//! internals; it consumes the stable `--no-tui` JSON and error contract.

use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

/// Upper bound for one CLI call; discovery, probes and updates can be slow.
const CLI_TIMEOUT: Duration = Duration::from_secs(600);
/// Error text forwarded from a CLI without a JSON envelope is truncated.
const MAX_ERROR_CHARS: usize = 4_000;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: u64 = 64 * 1024;

/// Drain both pipes concurrently, with bounded records and backpressure.
/// Redact complete lines so a secret split across OS reads cannot leak.
async fn forward_output(
    reader: impl AsyncRead + Unpin,
    channel: &'static str,
    events: &mpsc::Sender<Value>,
    secrets: &[ChildSecret],
) -> Result<String, CliFailure> {
    let mut reader = BufReader::new(reader);
    let mut output = String::new();
    loop {
        let mut line = Vec::new();
        let count = (&mut reader)
            .take(MAX_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|_| {
                CliFailure::panel(
                    "output_failed",
                    "Could not read command output".to_owned(),
                    Value::Null,
                )
            })?;
        if count == 0 {
            return Ok(output);
        }
        if count as u64 > MAX_LINE_BYTES || output.len() + count > MAX_OUTPUT_BYTES {
            return Err(CliFailure::panel(
                "output_limit",
                "Command output exceeded the panel limit".to_owned(),
                Value::Null,
            ));
        }
        let mut text = String::from_utf8_lossy(&line).into_owned();
        for secret in secrets.iter().filter(|secret| !secret.value.is_empty()) {
            // JSON diagnostics may quote/escape the submitted value.
            let quoted = json!(secret.value).to_string();
            text = text.replace(&quoted[1..quoted.len() - 1], "[redacted]");
            text = text.replace(&secret.value, "[redacted]");
        }
        output.push_str(&text);
        events
            .send(json!({"type": "output", "channel": channel, "text": text}))
            .await
            .map_err(|_| {
                CliFailure::panel(
                    "disconnected",
                    "Browser disconnected".to_owned(),
                    Value::Null,
                )
            })?;
    }
}

/// Unlike the legacy JSON collector, this owns cancellation until the entire
/// isolated process tree has been reaped, including a nested DUCX login.
pub(super) async fn run_cli_stream(
    args: &[String],
    secrets: &[ChildSecret],
    events: &mpsc::Sender<Value>,
    cancellation: &mut watch::Receiver<u64>,
) -> Result<String, CliFailure> {
    let executable = std::env::current_exe().map_err(|_| {
        CliFailure::panel(
            "panel_internal",
            "Could not locate the CLI".to_owned(),
            Value::Bool(false),
        )
    })?;
    let mut command = Command::new(executable);
    codex_mixin::platform::isolate_tokio_process_group(&mut command);
    command
        .args(["--no-tui", "--json-errors"])
        .args(args)
        .env("CODEX_MIXIN_LOGIN_STDIO", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for secret in secrets {
        command.env(&secret.env_name, &secret.value);
    }
    let mut child = command.spawn().map_err(|_| {
        CliFailure::panel(
            "panel_internal",
            "Could not start the CLI".to_owned(),
            Value::Bool(false),
        )
    })?;
    let pid = child.id();
    let result = async {
        let stdout = child.stdout.take().ok_or_else(|| {
            CliFailure::panel(
                "panel_internal",
                "Missing command output pipe".to_owned(),
                Value::Null,
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            CliFailure::panel(
                "panel_internal",
                "Missing command error pipe".to_owned(),
                Value::Null,
            )
        })?;
        let (stdout, stderr, status) = tokio::try_join!(
            forward_output(stdout, "stdout", events, secrets),
            forward_output(stderr, "stderr", events, secrets),
            async {
                child.wait().await.map_err(|_| {
                    CliFailure::panel(
                        "command_failed",
                        "Could not wait for the CLI".to_owned(),
                        Value::Null,
                    )
                })
            },
        )?;
        if !status.success() {
            return Err(CliFailure::from_stderr(&stderr));
        }
        Ok(stdout)
    };
    let result = tokio::select! {
        biased;
        _ = cancellation.changed() => Err(CliFailure::panel("cancelled", "Operation cancelled; refresh to check any completed steps".to_owned(), Value::Null)),
        _ = events.closed() => Err(CliFailure::panel("disconnected", "Browser disconnected; refresh to check any completed steps".to_owned(), Value::Null)),
        result = tokio::time::timeout(CLI_TIMEOUT, result) => result.unwrap_or_else(|_| Err(CliFailure::panel("timeout", "Command exceeded the 600s time limit; refresh to check any completed steps".to_owned(), Value::Null))),
    };
    // On success the CLI may deliberately start a managed daemon. Only aborts
    // kill descendants; otherwise reap the already-exited direct child.
    if result.is_err() {
        codex_mixin::platform::terminate_isolated_tokio_child(pid, &mut child).await;
    }
    result
}

/// Secret handed to the child through its environment and referenced in
/// argv as `@env:NAME`, so it never appears in a process listing or log.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct ChildSecret {
    pub(super) env_name: String,
    pub(super) value: String,
}

/// Failure in the CLI error-envelope shape (`code`, `message`, `stage`,
/// `committed`). Unknown commit state stays `null`; it never claims rollback.
#[derive(Debug)]
pub(super) struct CliFailure {
    pub(super) code: String,
    pub(super) message: String,
    pub(super) stage: Value,
    pub(super) committed: Value,
}

impl CliFailure {
    pub(super) fn panel(code: &str, message: String, committed: Value) -> Self {
        Self {
            code: code.to_owned(),
            message,
            stage: Value::Null,
            committed,
        }
    }

    pub(super) fn to_json(&self) -> Value {
        json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "stage": self.stage,
                "committed": self.committed,
            }
        })
    }

    /// Reads the envelope `--json-errors` writes on the final stderr line.
    pub(super) fn from_stderr(stderr: &str) -> Self {
        let envelope = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .and_then(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .and_then(|value| value.get("error").cloned())
            .filter(Value::is_object);
        if let Some(error) = envelope {
            return Self {
                code: error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("command_failed")
                    .to_owned(),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("command failed")
                    .to_owned(),
                stage: error.get("stage").cloned().unwrap_or(Value::Null),
                committed: error.get("committed").cloned().unwrap_or(Value::Null),
            };
        }
        let text = stderr.trim();
        let message = if text.is_empty() {
            "command failed without diagnostics".to_owned()
        } else {
            text.chars().take(MAX_ERROR_CHARS).collect()
        };
        Self::panel("command_failed", message, Value::Null)
    }
}

pub(super) async fn run_cli(
    args: &[String],
    secrets: &[ChildSecret],
) -> Result<String, CliFailure> {
    let executable = std::env::current_exe().map_err(|error| {
        CliFailure::panel(
            "panel_internal",
            format!("resolve current executable: {error}"),
            Value::Bool(false),
        )
    })?;
    let mut command = Command::new(&executable);
    command
        .arg("--no-tui")
        .arg("--json-errors")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Dropping the future on timeout or client disconnect stops the child.
        .kill_on_drop(true);
    for secret in secrets {
        command.env(&secret.env_name, &secret.value);
    }
    let label = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
    let output = match tokio::time::timeout(CLI_TIMEOUT, command.output()).await {
        Err(_) => {
            return Err(CliFailure::panel(
                "timeout",
                format!("`{label}` did not finish within {}s", CLI_TIMEOUT.as_secs()),
                Value::Null,
            ));
        }
        Ok(Err(error)) => {
            return Err(CliFailure::panel(
                "panel_internal",
                format!("run `{label}`: {error}"),
                Value::Bool(false),
            ));
        }
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        return Err(CliFailure::from_stderr(&String::from_utf8_lossy(
            &output.stderr,
        )));
    }
    String::from_utf8(output.stdout).map_err(|_| {
        CliFailure::panel(
            "invalid_output",
            format!("`{label}` printed non-UTF-8 output"),
            Value::Null,
        )
    })
}

pub(super) async fn run_json(args: &[&str]) -> Result<Value, CliFailure> {
    let owned = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let stdout = run_cli(&owned, &[]).await?;
    serde_json::from_str(&stdout).map_err(|error| {
        CliFailure::panel(
            "invalid_output",
            format!("parse `{}` output: {error}", args.join(" ")),
            Value::Null,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn output_redacts_split_and_json_escaped_secrets() {
        let secret = "fixture-\"secret\\value";
        let input = format!("{secret}\n{}\n", json!({"message": secret}));
        let (mut writer, reader) = tokio::io::duplex(8);
        let write = tokio::spawn(async move {
            for byte in input.bytes() {
                writer.write_all(&[byte]).await.unwrap();
            }
        });
        let (events, mut receiver) = mpsc::channel(4);
        let output = forward_output(
            reader,
            "stderr",
            &events,
            &[ChildSecret {
                env_name: "FIXTURE".to_owned(),
                value: secret.to_owned(),
            }],
        )
        .await
        .unwrap();
        write.await.unwrap();
        assert_eq!(output, "[redacted]\n{\"message\":\"[redacted]\"}\n");
        assert_eq!(receiver.recv().await.unwrap()["text"], "[redacted]\n");
        assert_eq!(
            receiver.recv().await.unwrap()["text"],
            "{\"message\":\"[redacted]\"}\n"
        );
    }

    #[tokio::test]
    async fn unterminated_output_is_bounded() {
        let input = vec![b'x'; MAX_LINE_BYTES as usize + 1];
        let (events, mut receiver) = mpsc::channel(1);
        let error = forward_output(input.as_slice(), "stdout", &events, &[])
            .await
            .unwrap_err();
        assert_eq!(error.code, "output_limit");
        assert!(error.committed.is_null());
        assert!(receiver.try_recv().is_err());
    }
}
