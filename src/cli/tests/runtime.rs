use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::cli::update::{cli_release_target, release_version_from_redirect, replace_executable};
use crate::cli::{atomic_file::*, runtime::*, service::*};

#[test]
fn rotates_gateway_log_at_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gateway.log");
    fs::write(&log, b"12345").unwrap();

    rotate_gateway_log_if_needed(&log, 5).unwrap();

    assert!(!log.exists());
    assert_eq!(
        fs::read(dir.path().join("gateway.log.1")).unwrap(),
        b"12345"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(dir.path().join("gateway.log.1"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn keeps_gateway_log_below_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gateway.log");
    fs::write(&log, b"1234").unwrap();

    rotate_gateway_log_if_needed(&log, 5).unwrap();

    assert_eq!(fs::read(log).unwrap(), b"1234");
    assert!(!dir.path().join("gateway.log.1").exists());
}

#[test]
fn outdated_gateway_runtime_is_replaced_on_its_existing_bind() {
    let legacy_runtime: RuntimeMetadata =
        serde_json::from_str(r#"{"pid":42,"bind":"127.0.0.1:18787","started_at":1}"#).unwrap();
    let older_runtime: RuntimeMetadata = serde_json::from_str(
        r#"{"pid":42,"bind":"127.0.0.1:18787","started_at":1,"version":"0.2.15"}"#,
    )
    .unwrap();
    let current_runtime: RuntimeMetadata = serde_json::from_value(serde_json::json!({
        "pid": 42,
        "bind": "127.0.0.1:18787",
        "started_at": 1,
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .unwrap();
    let existing_bind = "127.0.0.1:18787".parse().unwrap();

    assert_eq!(
        replacement_bind_for_outdated_runtime(&legacy_runtime, env!("CARGO_PKG_VERSION")),
        Some(existing_bind)
    );
    assert_eq!(
        replacement_bind_for_outdated_runtime(&older_runtime, env!("CARGO_PKG_VERSION")),
        Some(existing_bind)
    );
    assert_eq!(
        replacement_bind_for_outdated_runtime(&current_runtime, env!("CARGO_PKG_VERSION")),
        None
    );
}

#[test]
fn running_daemon_needs_replacement_when_config_or_arguments_change() {
    let runtime: RuntimeMetadata = serde_json::from_value(serde_json::json!({
        "pid": 42,
        "bind": "127.0.0.1:18787",
        "started_at": 1,
        "version": env!("CARGO_PKG_VERSION"),
    }))
    .unwrap();
    let daemon = DaemonMetadata {
        pid: 42,
        bind: "127.0.0.1:18787".parse().unwrap(),
        log_file: PathBuf::from("/tmp/gateway.log"),
        started_at: 1,
        config_fingerprint: Some(11),
    };

    assert!(!running_daemon_needs_replacement(
        &runtime,
        &daemon,
        runtime.bind,
        Path::new("/tmp/gateway.log"),
        Some(11),
    ));
    assert!(running_daemon_needs_replacement(
        &runtime,
        &daemon,
        runtime.bind,
        Path::new("/tmp/gateway.log"),
        Some(12),
    ));
    assert!(running_daemon_needs_replacement(
        &runtime,
        &daemon,
        "127.0.0.1:18788".parse().unwrap(),
        Path::new("/tmp/gateway.log"),
        Some(11),
    ));
    assert!(running_daemon_needs_replacement(
        &runtime,
        &daemon,
        runtime.bind,
        Path::new("/tmp/other.log"),
        Some(11),
    ));
}

#[test]
fn legacy_daemon_metadata_without_config_fingerprint_still_loads() {
    let daemon: DaemonMetadata = serde_json::from_str(
        r#"{"pid":42,"bind":"127.0.0.1:18787","log_file":"/tmp/gateway.log","started_at":1}"#,
    )
    .unwrap();
    assert_eq!(daemon.config_fingerprint, None);
    let runtime: RuntimeMetadata =
        serde_json::from_str(r#"{"pid":42,"bind":"127.0.0.1:18787","started_at":1}"#).unwrap();
    assert_eq!(runtime.config_fingerprint, None);
}

#[test]
fn release_version_parser_and_cli_target_are_available() {
    assert_eq!(
        release_version_from_redirect(
            "https://github.com/Edward-lyz/codex-mixin/releases/tag/v0.3.14"
        )
        .unwrap(),
        "0.3.14"
    );
    assert!(
        release_version_from_redirect("https://github.com/Edward-lyz/codex-mixin/releases/latest")
            .is_err()
    );

    let target = cli_release_target().unwrap();
    assert!(!target.is_empty());
    assert!(target.ends_with("apple-darwin") || target.ends_with("unknown-linux-musl"));
}

#[test]
fn replace_executable_swaps_the_target_without_leaving_backup_files() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("codex-mixin");
    let downloaded = directory.path().join("downloaded-codex-mixin");
    fs::write(&target, "old").unwrap();
    fs::write(&downloaded, "new").unwrap();

    replace_executable(&target, &downloaded).unwrap();

    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    let leftover = fs::read_dir(directory.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("codex-mixin-update-")
        });
    assert!(!leftover, "update staging and backup files must be removed");
}

#[cfg(unix)]
#[test]
fn atomic_rewrite_preserves_existing_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    assert!(write_atomic_if_changed(&path, b"new").unwrap());

    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
