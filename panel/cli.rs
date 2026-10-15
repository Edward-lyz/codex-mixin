//! Runs the real CLI as a child process. The panel never reaches CLI
//! internals; it consumes the stable `--no-tui` JSON and error contract.

use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::process::Command;

/// Upper bound for one CLI call; discovery, probes and updates can be slow.
const CLI_TIMEOUT: Duration = Duration::from_secs(600);
/// Error text forwarded from a CLI without a JSON envelope is truncated.
const MAX_ERROR_CHARS: usize = 4_000;

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
