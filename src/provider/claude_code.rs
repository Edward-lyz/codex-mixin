//! Claude Code plan allowance. The subscription token cannot call the usage
//! endpoint, so the allowance is the one Anthropic attaches to each reply the
//! official client receives; the latest values are kept for `quota`.
use anyhow::Context;
use serde_json::{Value, json};

const FILE: &str = "claude-code-limits";
const WINDOWS: [(&str, &str, &str); 2] = [("5h", "five_hour", "5h"), ("7d", "seven_day", "7d")];

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
