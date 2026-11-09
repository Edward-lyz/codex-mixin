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
