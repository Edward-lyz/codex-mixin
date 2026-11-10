//! Claude Code plan allowance and model list. The subscription token cannot
//! call the usage endpoint, so the allowance is the one Anthropic attaches to
//! each reply the official client receives; the latest values are kept for
//! `quota`. The model list comes from the signed-in CLI itself.
use std::time::Duration;

use anyhow::Context;
use serde_json::{Value, json};

use super::{ProviderModel, ProviderRuntime};

const FILE: &str = "claude-code-limits";
const WINDOWS: [(&str, &str, &str); 2] = [("5h", "five_hour", "5h"), ("7d", "seven_day", "7d")];
pub(crate) const CLI_NAME: &str = "claude";
/// Variables an enclosing Claude Code session sets for its children; the
/// CLI would otherwise treat itself as nested or reuse the parent's channel.
/// API-key variables would switch it from the subscription to API billing.
pub(crate) const INHERITED_ENV: [&str; 9] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
];
pub(crate) const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
/// The SDK handshake answers with the models the account can pick; it sends
/// no model request, so listing costs no allowance.
const INITIALIZE: &[u8] =
    b"{\"type\":\"control_request\",\"request_id\":\"mixin-models\",\"request\":{\"subtype\":\"initialize\"}}\n";
const MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// Separates the resolved version from its blurb in the CLI's description,
/// as in "Fable 5.2 \u{b7} Most capable ...".
const DESCRIPTION_SEPARATOR: &str = " \u{b7} ";

/// Models the signed-in `claude` CLI offers through its aliases, resolved to
/// the concrete ids `--model` accepts.
pub(crate) async fn discover_models(
    provider: &ProviderRuntime,
) -> anyhow::Result<Vec<ProviderModel>> {
    let program = crate::platform::agent_cli::find_command(CLI_NAME)
        .context("Claude Code CLI `claude` was not found; install it and sign in")?;
    let workdir = std::env::temp_dir().join("codex-mixin-claude-code");
    crate::platform::ensure_owner_only_dir(&workdir)
        .context("create the Claude Code working directory")?;
    let mut command = crate::platform::launcher_command(&program);
    command
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--setting-sources",
            "",
        ])
        .current_dir(&workdir);
    for name in INHERITED_ENV {
        command.env_remove(name);
    }
    let token = provider.definition().auth.api_key.trim();
    if !token.is_empty() {
        command.env(TOKEN_ENV, token);
    }
    crate::platform::prepare_background_command(&mut command);
    let outcome = tokio::task::spawn_blocking(move || {
        crate::platform::agent_cli::run_command_with_input(
            command,
            MODEL_LIST_TIMEOUT,
            "Claude Code model list",
            Some(INITIALIZE.to_vec()),
        )
    })
    .await
    .context("list Claude Code models")??;
    parse_models(&outcome.stdout)
}

fn parse_models(stdout: &str) -> anyhow::Result<Vec<ProviderModel>> {
    let response = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["type"] == "control_response")
        .context("Claude Code did not answer the model list handshake")?;
    let response = &response["response"];
    if response["subtype"] == "error" {
        anyhow::bail!(
            "Claude Code rejected the model list handshake: {}",
            response["error"].as_str().unwrap_or("unknown error")
        );
    }
    let entries = response["response"]["models"]
        .as_array()
        .context("Claude Code handshake reported no models")?;
    let mut models: Vec<ProviderModel> = Vec::new();
    for entry in entries {
        // Older CLIs report only the alias; keep entries that already name a model.
        let Some(id) = entry["resolvedModel"]
            .as_str()
            .or_else(|| {
                entry["value"]
                    .as_str()
                    .filter(|id| id.starts_with("claude-"))
            })
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if models.iter().any(|model| model.id == id) {
            continue;
        }
        let name = entry["description"]
            .as_str()
            .and_then(|description| description.split(DESCRIPTION_SEPARATOR).next())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map_or_else(|| id.to_owned(), |name| format!("Claude {name}"));
        models.push(super::spec::claude_code_model(id, &name));
    }
    anyhow::ensure!(
        !models.is_empty(),
        "Claude Code handshake listed no usable models"
    );
    Ok(models)
}

/// Remember the allowance headers of one reply; replies without any are ignored.
pub(crate) fn record_limits(headers: &Value) -> anyhow::Result<()> {
    let mut windows = serde_json::Map::new();
    for (prefix, key, _) in WINDOWS {
        let number = |suffix: &str| {
            headers
                .get(format!("anthropic-ratelimit-unified-{prefix}-{suffix}"))
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
        };
        if let Some(utilization) = number("utilization") {
            windows.insert(
                key.to_owned(),
                json!({"utilization": utilization, "reset": number("reset")}),
            );
        }
    }
    if windows.is_empty() {
        return Ok(());
    }
    let snapshot =
        json!({"captured_at_ms": chrono::Utc::now().timestamp_millis(), "windows": windows});
    crate::platform::oauth::save_oauth(
        &crate::platform::oauth::oauth_path(FILE),
        &serde_json::to_vec(&snapshot)?,
    )
    .context("save Claude Code allowance")
}

/// Rows in the shape `quota --json` prints for percentage windows.
pub fn fetch_claude_code_quota() -> anyhow::Result<Vec<Value>> {
    let path = crate::platform::oauth::oauth_path(FILE);
    let raw = std::fs::read(&path).map_err(|error| {
        anyhow::anyhow!(
            "no Claude allowance recorded yet; it appears after the first request through Mixin ({error})"
        )
    })?;
    let snapshot: Value = serde_json::from_slice(&raw).context("read Claude Code allowance")?;
    let captured = snapshot["captured_at_ms"]
        .as_i64()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .context("Claude Code allowance has no capture time")?;
    let now = chrono::Utc::now();
    let mut rows = Vec::new();
    for (_, key, label) in WINDOWS {
        let Some(window) = snapshot["windows"].get(key) else {
            continue;
        };
        let reset = window["reset"]
            .as_f64()
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds as i64, 0));
        // A window that reset after the capture has started over.
        let expired = reset.is_some_and(|reset| reset <= now);
        let used = if expired {
            0.0
        } else {
            (window["utilization"].as_f64().unwrap_or_default() * 100.0).clamp(0.0, 100.0)
        };
        rows.push(json!({
            "quota_id": key, "label": label, "currency": null, "unit": "percent",
            "source": "claude_code_headers", "value": used, "used": used, "limit": 100.0,
            "remaining": 100.0 - used,
            "reset_at": reset.filter(|_| !expired).map(|reset| reset.to_rfc3339()),
            "error": null, "stale_at": captured.to_rfc3339(),
        }));
    }
    anyhow::ensure!(
        !rows.is_empty(),
        "Claude Code allowance snapshot has no windows"
    );
    Ok(rows)
}
