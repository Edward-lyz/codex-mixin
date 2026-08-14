//! Managed DUCX install + login, isolated under `~/.codex-mixin/ducx`.
//!
//! Tracks Baidu's `baidu-cx` package. DUCX is only used to mint the native
//! `comate_custom_header`; it never touches the user's own `~/.baidu-cx`
//! install, config, or hooks.

use std::fs;
use std::io::IsTerminal;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use tokio::process::Command;

const DUCX_DOWNLOAD_BASE_URL: &str = "http://baidu-cc-client.bj.bcebos.com/baidu-cx";

pub(super) async fn ensure_managed_ducx() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is required to install managed DUCX")?;
    let isolated_home = home.join(".codex-mixin/ducx/home");
    let codex_home = isolated_home.join(".baidu-cx");
    let executable = codex_home.join("baidu-cx/bin/ducx");
    let active_install = codex_home.join("baidu-cx");
    let active_is_symlink = active_install
        .symlink_metadata()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false);
    if !executable.is_file() || !active_is_symlink {
        install_managed_ducx(&home, &codex_home, &executable).await?;
    }
    if ducx_is_logged_in(&isolated_home) {
        println!(
            "Managed DUCX authentication is ready: {}",
            executable.display()
        );
        return Ok(executable);
    }
    ensure!(
        std::io::stdin().is_terminal(),
        "DUCX login is required; rerun setup in an interactive terminal or run `HOME={} CODEX_HOME={} {} login`",
        isolated_home.display(),
        codex_home.display(),
        executable.display()
    );
    println!("\nDUCX login is required. Complete QR-code login in this terminal.");
    let mut child = Command::new(&executable)
        .arg("login")
        .env("HOME", &isolated_home)
        .env("CODEX_HOME", &codex_home)
        .env("DISABLE_DUCX_CLI_UPDATE", "1")
        .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start DUCX login")?;
    let mut waited = 0;
    let status = loop {
        if let Some(status) = child.try_wait().context("failed to monitor DUCX login")? {
            break status;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        waited += 5;
        println!("Still waiting for DUCX login ({waited}s elapsed); continue after scanning.");
    };
    ensure!(status.success(), "DUCX login failed with {status}");
    ensure!(
        ducx_is_logged_in(&isolated_home),
        "DUCX login completed without a valid authenticated session"
    );
    println!("DUCX authentication completed.");
    Ok(executable)
}

async fn install_managed_ducx(
    home: &Path,
    codex_home: &Path,
    executable: &Path,
) -> anyhow::Result<()> {
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => anyhow::bail!("managed DUCX is not available for architecture {other}"),
    };
    // macOS ships bsdtar without reliable zstd; use the universally extractable
    // bzip2 archive on darwin and the plain tar on linux.
    let (os, extension, tar_flag) = match std::env::consts::OS {
        "macos" => ("darwin", "tar.bz2", "-xjf"),
        "linux" => ("linux", "tar", "-xf"),
        other => anyhow::bail!("managed DUCX is not available for OS {other}"),
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    let version = client
        .get(format!(
            "{DUCX_DOWNLOAD_BASE_URL}/baidu_cx_latest_version.txt"
        ))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?
        .trim()
        .to_owned();
    ensure!(
        !version.is_empty()
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'),
        "DUCX latest version response is invalid"
    );
    let archive_name = format!("baidu-cx-{os}-{architecture}-{version}.{extension}");
    let archive_url = format!("{DUCX_DOWNLOAD_BASE_URL}/{archive_name}");
    let download_dir = home.join(".codex-mixin/ducx/downloads");
    fs::create_dir_all(&download_dir)?;
    fs::set_permissions(&download_dir, fs::Permissions::from_mode(0o700))?;
    let archive_path = download_dir.join(&archive_name);
    println!("Downloading managed DUCX {version} for {os} {architecture}...");
    let status = Command::new("curl")
        .args(["--fail", "--location", "--progress-bar", "--output"])
        .arg(&archive_path)
        .arg(&archive_url)
        .status()
        .await
        .context("failed to download the managed DUCX archive with curl")?;
    ensure!(
        status.success(),
        "failed to download managed DUCX: {archive_url}"
    );
    ensure!(
        fs::metadata(&archive_path)?.len() > 20 * 1024 * 1024,
        "managed DUCX archive is unexpectedly small: {}",
        archive_path.display()
    );

    let root = codex_home;
    let version_dir_name = format!("baidu-cx-{os}-{architecture}-{version}");
    let version_dir = root.join(&version_dir_name);
    fs::create_dir_all(&version_dir)?;
    let status = Command::new("tar")
        .arg(tar_flag)
        .arg(&archive_path)
        .arg("-C")
        .arg(&version_dir)
        .status()
        .await
        .context("failed to extract managed DUCX archive with tar")?;
    ensure!(status.success(), "failed to extract managed DUCX archive");
    // The archive ships `bin/codex`; the official installer creates the `ducx`
    // entry as a symlink to it. Mirror that so `bin/ducx` exists.
    let bin_dir = version_dir.join("bin");
    let codex_bin = bin_dir.join("codex");
    ensure!(
        codex_bin.is_file(),
        "managed DUCX archive is missing bin/codex at {}",
        codex_bin.display()
    );
    fs::set_permissions(&codex_bin, fs::Permissions::from_mode(0o755))?;
    let ducx_bin = bin_dir.join("ducx");
    if ducx_bin.symlink_metadata().is_ok() {
        fs::remove_file(&ducx_bin).ok();
    }
    symlink("codex", &ducx_bin)?;
    // Drop any bundled config/auth/hooks so the isolated install starts clean.
    for name in ["config.toml", "auth.json", "hooks.json", "user.json"] {
        let _ = fs::remove_file(version_dir.join(name));
    }
    let active_install = root.join("baidu-cx");
    if active_install.symlink_metadata().is_ok() {
        fs::remove_file(&active_install).ok();
    }
    fs::create_dir_all(root)?;
    symlink(&version_dir_name, &active_install)?;
    ensure!(
        executable.is_file(),
        "managed DUCX archive is missing bin/ducx at {}",
        executable.display()
    );
    println!("Managed DUCX installed: {}", executable.display());
    Ok(())
}

fn ducx_is_logged_in(isolated_home: &Path) -> bool {
    // DUCX writes the signed-in identity under HOME/.comate/login-user/<username>.
    let login_dir = isolated_home.join(".comate/login-user");
    fs::read_dir(&login_dir)
        .map(|entries| {
            let files = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .take(2)
                .collect::<Vec<_>>();
            files.len() == 1
                && fs::read_to_string(files[0].path())
                    .is_ok_and(|token| is_valid_login_token(token.trim()))
        })
        .unwrap_or(false)
}

fn is_valid_login_token(token: &str) -> bool {
    let token = token
        .strip_prefix("Bearer-")
        .or_else(|| token.strip_prefix("Bearer "))
        .unwrap_or(token);
    let mut segments = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return false;
    };
    if signature.is_empty() {
        return false;
    }
    let Ok(header) = URL_SAFE_NO_PAD.decode(header) else {
        return false;
    };
    let Ok(payload) = URL_SAFE_NO_PAD.decode(payload) else {
        return false;
    };
    let Ok(header) = serde_json::from_slice::<Value>(&header) else {
        return false;
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&payload) else {
        return false;
    };
    if header.get("alg").and_then(Value::as_str).is_none()
        || payload.get("sub").and_then(Value::as_str).is_none()
        || payload.get("iat").and_then(Value::as_i64).is_none()
    {
        return false;
    }
    match payload.get("exp").and_then(Value::as_i64) {
        None => true,
        Some(expires_at) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .is_ok_and(|now| expires_at > now.as_secs() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::{ducx_is_logged_in, is_valid_login_token};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::fs;

    #[test]
    fn login_state_is_scoped_to_isolated_home() {
        let root = tempfile::tempdir().unwrap();
        assert!(!ducx_is_logged_in(root.path()));
        let login_dir = root.path().join(".comate/login-user");
        fs::create_dir_all(&login_dir).unwrap();
        fs::write(login_dir.join("liyanzhen01"), valid_token()).unwrap();
        assert!(ducx_is_logged_in(root.path()));
        fs::write(login_dir.join("another-user"), b"{}").unwrap();
        assert!(!ducx_is_logged_in(root.path()));
    }

    #[test]
    fn rejects_stale_login_markers() {
        assert!(!is_valid_login_token(""));
        assert!(!is_valid_login_token("{}"));
        assert!(!is_valid_login_token("header.payload.signature"));
    }

    #[test]
    fn accepts_the_bearer_jwt_format_written_by_ducx() {
        assert!(is_valid_login_token(&format!("Bearer-{}", valid_token())));
        assert!(is_valid_login_token(&format!("Bearer {}", valid_token())));
    }

    fn valid_token() -> String {
        let encode = |value: &str| URL_SAFE_NO_PAD.encode(value.as_bytes());
        format!(
            "{}.{}.signature",
            encode(r#"{"alg":"RS256"}"#),
            encode(r#"{"sub":"user","iat":1786600000}"#)
        )
    }
}
