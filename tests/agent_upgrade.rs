//! Agent upgrade failures must reach callers through the CLI error envelope.

use serde_json::Value;

mod common;

#[test]
fn a_desktop_only_agent_cannot_report_a_successful_cli_upgrade() {
    let directory = tempfile::tempdir().unwrap();
    let result = common::cli(directory.path())
        .args([
            "--no-tui",
            "--json-errors",
            "agent",
            "upgrade",
            "cursor-local",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cursor-local")
    );
}

#[cfg(unix)]
#[test]
fn a_successful_updater_exit_without_a_version_change_is_unverified() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let prefix = directory.path().join("npm");
    let bin = prefix.join("bin");
    let package = prefix.join("lib/node_modules/@openai/codex");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&package).unwrap();
    fs::copy("/usr/bin/true", package.join("codex")).unwrap();
    fs::write(package.join("package.json"), r#"{"version":"0.1.0"}"#).unwrap();
    symlink(package.join("codex"), bin.join("codex")).unwrap();
    symlink("/usr/bin/true", bin.join("npm")).unwrap();

    // The isolated CLI has no registry access. A zero exit plus the old
    // version cannot confirm that the package was upgraded or up to date.
    let result = common::cli(directory.path())
        .env("PATH", bin)
        .args([
            "--no-tui",
            "--json-errors",
            "agent",
            "upgrade",
            "codex",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    let error: Value = serde_json::from_slice(&result.stderr).unwrap();
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.contains("codex reports 0.1.0 after upgrade"));
}
