use std::fs;
use std::io::IsTerminal;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, ensure};
use serde::Deserialize;

const DUCC_DOWNLOAD_BASE_URL: &str = "http://baidu-cc-client.bj.bcebos.com/baidu-cc";

#[derive(Deserialize)]
struct DuccAuthStatus {
    #[serde(rename = "loggedIn")]
    logged_in: bool,
}

pub(super) async fn ensure_managed_ducc() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is required to install managed DUCC")?;
    let isolated_home = home.join(".codex-mixin/ducc/home");
    let executable = isolated_home.join(".baidu-cc/baidu-cc/bin/ducc");
    let active_install = isolated_home.join(".baidu-cc/baidu-cc");
    let active_is_symlink = active_install
        .symlink_metadata()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false);
    if !executable.is_file() || !active_is_symlink {
        install_managed_ducc(&home, &isolated_home, &executable).await?;
    }
    let login_executable = active_install
        .read_link()
        .map(|target| {
            isolated_home
                .join(".baidu-cc")
                .join(target)
                .join("bin/claude")
        })
        .unwrap_or_else(|_| active_install.join("bin/claude"));
    if ducc_is_logged_in(&executable, &isolated_home, true)? {
        println!("DUCC authentication is ready: {}", executable.display());
        return Ok(executable);
    }
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "DUCC login is required; rerun setup in an interactive terminal or run `HOME={} {} login`",
            isolated_home.display(),
            executable.display()
        )
    }
    println!("\nDUCC login is required.");
    println!("Managed HOME: {}", isolated_home.display());
    println!("Login executable: {}", executable.display());
    println!(
        "Complete QR-code login in this terminal; if no QR code appears within 10s, check that the terminal supports interactive input."
    );
    let mut child = Command::new(&login_executable)
        .arg("login")
        .env("HOME", &isolated_home)
        .env_remove("DISABLE_BAIDU_CLAUDE_UPDATE")
        .env_remove("DISABLE_DUCC_CLI_UPDATE")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start DUCC login")?;
    println!("DUCC login process started (pid {}).", child.id());
    let mut waited = 0;
    let status = loop {
        if let Some(status) = child.try_wait().context("failed to monitor DUCC login")? {
            break status;
        }
        thread::sleep(Duration::from_secs(5));
        waited += 5;
        println!(
            "Still waiting for DUCC login ({waited}s elapsed); continue after scanning, or press Ctrl-C to cancel."
        );
    };
    ensure!(status.success(), "DUCC login failed with {status}");
    ensure!(
        ducc_is_logged_in(&executable, &isolated_home, true)?,
        "DUCC login completed without a valid authenticated session"
    );
    println!("DUCC authentication completed; continuing provider setup.");
    Ok(executable)
}

async fn install_managed_ducc(
    home: &Path,
    isolated_home: &Path,
    executable: &Path,
) -> anyhow::Result<()> {
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => anyhow::bail!("managed DUCC is not available for Linux architecture {other}"),
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    let version = client
        .get(format!(
            "{DUCC_DOWNLOAD_BASE_URL}/baidu_cc_latest_version.txt"
        ))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let version = version.trim();
    ensure!(
        !version.is_empty()
            && version
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '.'
                    || character == '-'),
        "DUCC latest version response is invalid"
    );
    let archive_name = format!("baidu-cc-linux-{architecture}-{version}.tar");
    let archive_url = format!("{DUCC_DOWNLOAD_BASE_URL}/{archive_name}");
    let download_dir = home.join(".codex-mixin/ducc/downloads");
    fs::create_dir_all(&download_dir)?;
    fs::set_permissions(&download_dir, fs::Permissions::from_mode(0o700))?;
    let archive_path = download_dir.join(&archive_name);
    println!("Downloading managed DUCC {version} for Linux {architecture} (about 300 MB)...");
    let status = Command::new("curl")
        .args(["--fail", "--location", "--progress-bar", "--output"])
        .arg(&archive_path)
        .arg(&archive_url)
        .status()
        .context("failed to download the managed DUCC archive with curl")?;
    ensure!(
        status.success(),
        "failed to download managed DUCC: {archive_url}"
    );
    ensure!(
        fs::metadata(&archive_path)?.len() > 100 * 1024 * 1024,
        "managed DUCC archive is unexpectedly small: {}",
        archive_path.display()
    );
    fs::set_permissions(&archive_path, fs::Permissions::from_mode(0o600))?;

    let ducc_root = isolated_home.join(".baidu-cc");
    let version_dir_name = format!("baidu-cc-linux-{architecture}-{version}");
    let version_dir = ducc_root.join(&version_dir_name);
    fs::create_dir_all(&version_dir)?;
    let status = Command::new("tar")
        .args(["-xf"])
        .arg(&archive_path)
        .arg("-C")
        .arg(&version_dir)
        .status()
        .context("failed to extract managed DUCC archive with tar")?;
    ensure!(status.success(), "failed to extract managed DUCC archive");
    let active_install = ducc_root.join("baidu-cc");
    if active_install.symlink_metadata().is_ok() {
        let backup = ducc_root.join(format!(
            "baidu-cc.backup-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs()
        ));
        fs::rename(&active_install, backup)?;
    }
    symlink(&version_dir_name, &active_install)?;
    let bootstrap_executable = active_install.join("bin/claude");
    ensure!(
        bootstrap_executable.is_file(),
        "managed DUCC archive is missing bin/claude"
    );
    fs::set_permissions(&bootstrap_executable, fs::Permissions::from_mode(0o700))?;
    let ducc_launcher = executable
        .parent()
        .context("managed DUCC executable has no bin directory")?;
    let launcher = format!(
        "#!/bin/sh\nset -eu\nexport DISABLE_BAIDU_CLAUDE_UPDATE=1\nexport DISABLE_DUCC_CLI_UPDATE=1\nexec \"{}\" \"$@\"\n",
        bootstrap_executable.display()
    );
    fs::write(&executable, launcher)?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    ensure!(
        ducc_launcher.is_dir(),
        "managed DUCC bin directory was not installed"
    );
    println!("Managed DUCC installed: {}", executable.display());
    Ok(())
}

fn ducc_is_logged_in(
    executable: &Path,
    isolated_home: &Path,
    disable_updates: bool,
) -> anyhow::Result<bool> {
    let mut command = Command::new(executable);
    command.args(["auth", "status"]).env("HOME", isolated_home);
    if disable_updates {
        command
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .env("DISABLE_DUCC_CLI_UPDATE", "1");
    }
    let output = command
        .output()
        .context("failed to query DUCC authentication status")?;
    if !output.status.success() {
        return Ok(false);
    }
    let status: DuccAuthStatus = serde_json::from_slice(&output.stdout)
        .context("DUCC authentication status returned invalid JSON")?;
    Ok(status.logged_in)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ducc_authentication_status() {
        let status: DuccAuthStatus = serde_json::from_str(r#"{"loggedIn":true}"#).unwrap();
        assert!(status.logged_in);
    }
}
