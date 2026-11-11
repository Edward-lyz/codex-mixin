use std::path::Path;

use serde_json::Value;

mod common;

const KIRO_TOKEN: &str = r#"{"refreshToken":"kiro-refresh","profileArn":"arn:test"}"#;

fn write_login(root: &Path, relative: &str, body: &str) -> std::path::PathBuf {
    let path = root.join("home").join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    path
}

fn list(root: &Path) -> Value {
    let output = common::cli(root)
        .args(["--no-tui", "--json-errors", "providers", "list", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn provider<'a>(listing: &'a Value, id: &str) -> Option<&'a Value> {
    listing["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["id"] == id)
}

#[test]
fn listing_shows_a_local_login_without_writing_anything() {
    let root = tempfile::tempdir().unwrap();
    let token = write_login(
        root.path(),
        ".aws/sso/cache/kiro-auth-token.json",
        KIRO_TOKEN,
    );

    let listing = list(root.path());

    let kiro = provider(&listing, "kiro").expect("kiro login should be listed");
    assert_eq!(kiro["source"], "auto_discovered");
    assert_eq!(
        kiro["last_model_refresh_error"],
        "model directory pending refresh"
    );
    assert_eq!(std::fs::read_to_string(token).unwrap(), KIRO_TOKEN);
    assert!(
        !root.path().join("config.json").exists(),
        "listing is read-only and must not save the discovered provider"
    );
}

#[test]
fn business_and_broken_logins_are_not_adopted() {
    let root = tempfile::tempdir().unwrap();
    write_login(
        root.path(),
        ".gemini/antigravity-cli/antigravity-oauth-token",
        r#"{"auth_method":"gcp","token":{"access_token":"a","refresh_token":"r"}}"#,
    );
    write_login(
        root.path(),
        ".aws/sso/cache/kiro-auth-token.json",
        "{broken",
    );

    let listing = list(root.path());

    assert!(provider(&listing, "agy").is_none());
    assert!(provider(&listing, "kiro").is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn desktop_discovers_claude_setup_token_from_login_shell() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    write_login(
        root.path(),
        ".zshrc",
        "export CLAUDE_CODE_OAUTH_TOKEN=fixture-setup-token\n",
    );
    let bin = root.path().join("home/.local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cli = bin.join("claude");
    std::fs::write(&cli, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = common::cli(root.path())
        .env("SHELL", "/bin/zsh")
        .args(["--no-tui", "providers", "list", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        provider(&listing, "claude-code").is_some(),
        "desktop process must discover the shell's setup token"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-setup-token"));
}

/// A profile hook that never returns (a tool blocked on the OS) must not
/// outlive the bounded login-shell read: the timeout ends the shell's whole
/// process group, not only the shell.
#[cfg(unix)]
#[test]
fn hung_login_shell_leaves_no_helper_process_behind() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    struct Helper(String);
    impl Helper {
        fn running(&self) -> bool {
            // A zombie still answers `kill -0`; only a live state counts.
            Command::new("ps")
                .args(["-o", "stat=", "-p", &self.0])
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
                .is_ok_and(|state| !state.is_empty() && !state.starts_with('Z'))
        }
    }
    impl Drop for Helper {
        fn drop(&mut self) {
            if self.running() {
                let _ = Command::new("kill").args(["-9", &self.0]).status();
            }
        }
    }

    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("helper.pid");
    // The "shell" starts a helper like a hook in the user's profile, records
    // its PID and waits for it forever.
    let shell = root.path().join("hung-shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nsleep 600 &\necho $! > '{}'\nwait\n",
            pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();

    let output = common::cli(root.path())
        .env("SHELL", &shell)
        .args(["--no-tui", "providers", "list", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let helper = Helper(
        std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .to_owned(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while helper.running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !helper.running(),
        "the timed-out login shell left its helper process running"
    );
}
