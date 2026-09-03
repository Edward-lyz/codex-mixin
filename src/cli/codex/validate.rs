//! Post-install validation: ask the Codex CLI itself to load the managed
//! config and catalog, and confirm it sees what we wrote.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command as ProcessCommand;

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
    let doctor = ProcessCommand::new(codex_cli)
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
    let config_check = doctor_report
        .pointer("/checks/config.load")
        .ok_or_else(|| anyhow::anyhow!("Codex doctor report has no config.load check"))?;
    let config_status = config_check
        .get("status")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Codex config.load check has no status: {config_check}"))?;
    if !codex_config_load_status_is_acceptable(Some(config_status)) {
        anyhow::bail!("Codex config.load check failed: {config_check}");
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

    let models = ProcessCommand::new(codex_cli)
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
