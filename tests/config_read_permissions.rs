//! Reading the provider configuration must not rewrite the state directory
//! permissions. Windows expands an inheritable private DACL to every existing
//! descendant, so doing that on every `provider list` walks agent session trees
//! with hundreds of thousands of entries and blocks GUI queries for minutes.

use std::fs;
use std::path::Path;

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::CONFIG_VERSION;

mod common;

#[test]
fn provider_list_keeps_state_directory_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            config_version: CONFIG_VERSION,
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();

    grant_other_users_read(directory.path());
    let before = acl_snapshot(directory.path());

    let output = common::cli(directory.path())
        .args(["providers", "list", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "provider list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        acl_snapshot(directory.path()),
        before,
        "reading the configuration must not rewrite state directory permissions"
    );
}

#[cfg(unix)]
fn grant_other_users_read(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn acl_snapshot(path: &Path) -> Vec<u8> {
    use std::os::unix::fs::PermissionsExt;

    format!("{:o}", fs::metadata(path).unwrap().permissions().mode()).into_bytes()
}

#[cfg(windows)]
fn grant_other_users_read(path: &Path) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/grant", "*S-1-5-32-545:R"])
        .status()
        .expect("run icacls");
    assert!(status.success(), "icacls could not grant Users read access");
}

#[cfg(windows)]
fn acl_snapshot(path: &Path) -> Vec<u8> {
    let snapshot = path.join("acl-snapshot.txt");
    let _ = fs::remove_file(&snapshot);
    let status = std::process::Command::new("icacls")
        .arg(path)
        .arg("/save")
        .arg(&snapshot)
        .status()
        .expect("run icacls");
    assert!(status.success(), "icacls could not snapshot the DACL");
    let acl = fs::read(&snapshot).unwrap();
    let _ = fs::remove_file(&snapshot);
    acl
}
