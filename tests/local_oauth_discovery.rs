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
