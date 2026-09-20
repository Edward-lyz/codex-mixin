//! Post-install validation: ask the Codex CLI itself to load the managed
//! config and catalog, and confirm it sees what we wrote.

use std::collections::HashSet;
use std::path::Path;

use super::bin::codex_command;
pub(in crate::cli) fn validate_codex_install(
    codex_cli: &Path,
    codex_home: &Path,
    expected_provider: &str,
    expected_model_slugs: &[String],
) -> anyhow::Result<()> {
    println!(
        "codex validation started: cli={}, codex_home={}, expected_models={}",
        codex_cli.display(),
        codex_home.display(),
        expected_model_slugs.len()
    );
    let doctor = codex_command(codex_cli)
        .args(["doctor", "--json"])
        .env("CODEX_HOME", codex_home)
        .output()?;
    let doctor_report: serde_json::Value =
        serde_json::from_slice(&doctor.stdout).map_err(|error| {
            anyhow::anyhow!(
                "Codex doctor returned invalid JSON: {error}; stderr: {}",
                String::from_utf8_lossy(&doctor.stderr)
                    .chars()
                    .take(1000)
                    .collect::<String>()
            )
        })?;
    let config_check = find_codex_config_load_check(&doctor_report).ok_or_else(|| {
        anyhow::anyhow!("Codex doctor report has no config.load check: {doctor_report}")
    })?;
    let config_status = config_check
        .get("status")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Codex config.load check has no status: {config_check}"))?;
    if !codex_config_load_status_is_acceptable(Some(config_status)) {
        // `codex doctor` reports config.load failures with an empty `details`
        // object, so re-run `codex debug models` which prints the concrete
        // deserialize/validation error (e.g. an unsupported provider field or a
        // catalog schema mismatch) to stderr, and surface that reason.
        println!("codex validation: doctor report={doctor_report}");
        let reason = codex_command(codex_cli)
            .args(["debug", "models"])
            .env("CODEX_HOME", codex_home)
            .output()
            .ok()
            .map(|output| {
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .find(|line| line.contains("Error") || line.contains("error"))
                    .unwrap_or("")
                    .chars()
                    .take(1000)
                    .collect::<String>()
            })
            .filter(|reason| !reason.trim().is_empty());
        match reason {
            Some(reason) => anyhow::bail!(
                "Codex config.load check failed: {}; codex reported: {reason}",
                describe_config_check(config_check)
            ),
            None => anyhow::bail!(
                "Codex config.load check failed: {}",
                describe_config_check(config_check)
            ),
        }
    }
    if config_status == "warning" {
        let warning_count = config_check
            .get("details")
            .and_then(|details| details.get("startup warnings"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        println!(
            "codex validation: doctor config.load warning accepted; startup_warnings={warning_count}"
        );
    }
    let effective_provider = config_check
        .pointer("/details/model provider")
        .and_then(serde_json::Value::as_str);
    if effective_provider != Some(expected_provider) {
        anyhow::bail!(
            "Codex loaded model provider {:?}, expected {expected_provider}",
            effective_provider
        );
    }
    println!("codex validation: doctor config.load {config_status}; provider={expected_provider}");

    let models = codex_command(codex_cli)
        .args(["debug", "models"])
        .env("CODEX_HOME", codex_home)
        .output()?;
    if !models.status.success() {
        anyhow::bail!(
            "Codex failed to load the managed model catalog: {}",
            String::from_utf8_lossy(&models.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
        );
    }
    let loaded_catalog: serde_json::Value = serde_json::from_slice(&models.stdout)
        .map_err(|error| anyhow::anyhow!("Codex model catalog output is invalid JSON: {error}"))?;
    let loaded_slugs = loaded_catalog
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Codex model catalog output has no models array"))?
        .iter()
        .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
        .collect::<HashSet<_>>();
    let missing_slugs = expected_model_slugs
        .iter()
        .filter(|slug| !loaded_slugs.contains(slug.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    println!(
        "codex validation: debug models loaded {} models; expected {}; missing {}",
        loaded_slugs.len(),
        expected_model_slugs.len(),
        missing_slugs.len()
    );
    if !missing_slugs.is_empty() {
        anyhow::bail!(
            "Codex did not load {} managed models: {}",
            missing_slugs.len(),
            missing_slugs.join(", ")
        );
    }
    Ok(())
}

pub(in crate::cli) fn codex_config_load_status_is_acceptable(status: Option<&str>) -> bool {
    matches!(status, Some("ok" | "warning"))
}

/// Produce a compact, human-readable reason from a doctor `config.load` check so
/// the surfaced error explains *why* Codex refused the config instead of dumping
/// the whole JSON node into the UI.
fn describe_config_check(check: &serde_json::Value) -> String {
    let status = check
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let detail = check
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            check
                .get("details")
                .filter(|details| !details.is_null())
                .map(|details| match details {
                    serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                        details.to_string()
                    }
                    other => other.to_string(),
                })
        })
        .unwrap_or_default();
    if detail.is_empty() || detail == "{}" || detail == "null" {
        // Empty message/details are useless for diagnosis; fall back to the whole
        // check node so the surfaced error still carries Codex's real reason.
        format!("status={status}; {check}")
    } else {
        format!("status={status}; {detail}")
    }
}

pub(in crate::cli) fn find_codex_config_load_check(
    report: &serde_json::Value,
) -> Option<&serde_json::Value> {
    if let Some(check) = report.pointer("/checks/config.load") {
        return Some(check);
    }
    let checks = report.get("checks")?;
    if let Some(object) = checks.as_object() {
        for (key, value) in object {
            if key == "config.load" || key.ends_with("config.load") {
                return Some(value);
            }
        }
    }
    if let Some(array) = checks.as_array() {
        for check in array {
            let id = check
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if id == "config.load" || id.ends_with("config.load") {
                return Some(check);
            }
        }
    }
    None
}
