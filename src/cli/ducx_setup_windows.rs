use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, ensure};
use reqwest::Client;
use tokio::process::Command;

const DOWNLOAD_BASE_URL: &str = "http://baidu-cc-client.bj.bcebos.com/baidu-cx";

pub(super) async fn ensure_managed_ducx() -> anyhow::Result<PathBuf> {
    let home =
        windows_home().context("Windows user profile is required to install managed DUCX")?;
    let install_home = home.join(".codex-mixin/ducx/home");
    let executable = install_home.join(".baidu-cx/baidu-cx/bin/ducx.exe");
    if executable.is_file() {
        ensure_logged_in(&executable, &install_home).await?;
        return Ok(executable);
    }

    super::progress_step("Preparing DUCX authentication");
    install_native(&install_home, &executable).await?;
    ensure!(
        executable.is_file(),
        "Windows DUCX installer completed without creating {}",
        executable.display()
    );
    ensure_logged_in(&executable, &install_home).await?;
    Ok(executable)
}

async fn install_native(install_home: &Path, executable: &Path) -> anyhow::Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("create DUCX download client")?;
    let version = client
        .get(format!("{DOWNLOAD_BASE_URL}/baidu_cx_latest_version.txt"))
        .send()
        .await
        .context("download DUCX version")?
        .error_for_status()
        .context("DUCX version request failed")?
        .text()
        .await
        .context("read DUCX version")?
        .trim()
        .to_owned();
    ensure!(
        !version.is_empty()
            && version
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || value == '.' || value == '-'),
        "DUCX version response is invalid"
    );
    let archive_name = format!("baidu-cx-windows-amd64-{version}.tar.bz2");
    let download_dir = install_home.join("downloads");
    tokio::fs::create_dir_all(&download_dir).await?;
    let archive = download_dir.join(&archive_name);
    let bytes = client
        .get(format!("{DOWNLOAD_BASE_URL}/{archive_name}"))
        .send()
        .await
        .context("download Windows DUCX archive")?
        .error_for_status()
        .context("Windows DUCX archive request failed")?
        .bytes()
        .await
        .context("read Windows DUCX archive")?;
    ensure!(!bytes.is_empty(), "Windows DUCX archive is empty");
    tokio::fs::write(&archive, &bytes).await?;

    let root = install_home.join(".baidu-cx");
    let version_dir = root.join(format!("baidu-cx-windows-amd64-{version}"));
    tokio::fs::create_dir_all(&version_dir).await?;
    let archive_path = archive.clone();
    let extract_dir = version_dir.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let archive = fs::File::open(&archive_path)
            .with_context(|| format!("open Windows DUCX archive {}", archive_path.display()))?;
        let decoder = bzip2::read::BzDecoder::new(archive);
        tar::Archive::new(decoder)
            .unpack(&extract_dir)
            .with_context(|| format!("extract Windows DUCX archive to {}", extract_dir.display()))
    })
    .await
    .context("join Windows DUCX archive extraction")??;
    let active = root.join("baidu-cx");
    if active.exists() {
        tokio::fs::remove_dir_all(&active).await?;
    }
    copy_dir_all(&version_dir, &active).await?;
    let codex = active.join("bin/baidu-codex.exe");
    ensure!(
        codex.is_file(),
        "Windows DUCX archive is missing bin/baidu-codex.exe"
    );
    if !executable.is_file() {
        tokio::fs::copy(&codex, executable).await?;
    }
    for name in ["config.toml", "auth.json", "hooks.json"] {
        let _ = tokio::fs::remove_file(active.join(name)).await;
    }
    Ok(())
}

async fn copy_dir_all(source: &Path, target: &Path) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(target).await?;
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type().await?.is_dir() {
            Box::pin(copy_dir_all(&source_path, &target_path)).await?;
        } else {
            tokio::fs::copy(source_path, target_path).await?;
        }
    }
    Ok(())
}

async fn ensure_logged_in(executable: &Path, home: &Path) -> anyhow::Result<()> {
    if is_logged_in(home) {
        return Ok(());
    }
    let codex_home = home.join(".baidu-cx");
    // A GUI-launched gateway has no console for the QR-code login, so open a
    // dedicated console window running `ducx login` and wait for the login
    // state to appear. An interactive terminal (CLI/TUI) logs in inline.
    if std::io::stdin().is_terminal() {
        super::progress_step("Waiting for DUCX login");
        let mut child = Command::new(executable)
            .arg("login")
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("CODEX_HOME", &codex_home)
            .env("DISABLE_DUCX_CLI_UPDATE", "1")
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to start Windows DUCX login")?;
        let status = child
            .wait()
            .await
            .context("failed to monitor Windows DUCX login")?;
        ensure!(status.success(), "Windows DUCX login failed with {status}");
    } else {
        super::progress_step("Waiting for DUCX login");
        // DUCX draws the login QR with Unicode half-block glyphs. The legacy
        // console (conhost) renders those with the wrong cell aspect ratio, so
        // the QR comes out distorted and unscannable even under UTF-8. Windows
        // Terminal (wt.exe) renders the blocks crisply and square, so prefer it
        // when available. The launcher script switches to UTF-8 first, then runs
        // `ducx login`; using a file avoids fragile nested-quote parsing.
        let launcher = home.join("ducx-login.bat");
        // Set HOME/CODEX_HOME inside the script rather than only as process env:
        // wt.exe hands the command to an already-running Windows Terminal host,
        // so env vars set on the wt.exe process are NOT inherited by the new
        // tab. Exporting them in the script guarantees DUCX writes its login
        // state into the isolated managed home that `is_logged_in` polls.
        let script = format!(
            "@echo off\r\nchcp 65001 >nul\r\nset \"HOME={home}\"\r\nset \"USERPROFILE={home}\"\r\nset \"CODEX_HOME={codex_home}\"\r\nset \"DISABLE_DUCX_CLI_UPDATE=1\"\r\nset \"DISABLE_BAIDU_CLAUDE_UPDATE=1\"\r\n\"{exe}\" login\r\n",
            home = home.display(),
            codex_home = codex_home.display(),
            exe = executable.display()
        );
        tokio::fs::write(&launcher, script)
            .await
            .context("write DUCX login launcher")?;
        if let Some(wt) = windows_terminal() {
            // wt.exe returns immediately (it signals the running terminal), so
            // poll the login state below instead of waiting on the process.
            // `-w new` forces a brand-new Windows Terminal window (instead of a
            // tab in an existing, possibly background/minimized host) so the QR
            // window is created in the foreground and the user actually notices
            // it.
            Command::new(wt)
                .arg("-w")
                .arg("new")
                .arg("--title")
                .arg("百度 DUCX 登录")
                .arg("cmd")
                .arg("/c")
                .arg(&launcher)
                .env("HOME", home)
                .env("CODEX_HOME", &codex_home)
                .env("DISABLE_DUCX_CLI_UPDATE", "1")
                .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
                .spawn()
                .context("failed to open the DUCX login window in Windows Terminal")?;
        } else {
            // Fall back to the legacy console. The QR may render less crisply,
            // but the flow still works. `/wait` blocks until the window closes.
            Command::new("cmd")
                .arg("/c")
                .arg("start")
                .arg("百度 DUCX 登录")
                .arg("/wait")
                .arg(&launcher)
                .env("HOME", home)
                .env("CODEX_HOME", &codex_home)
                .env("DISABLE_DUCX_CLI_UPDATE", "1")
                .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
                .status()
                .await
                .context("failed to open the DUCX login window")?;
        }
        // Poll the login state for up to five minutes so a slow QR scan still
        // completes the flow instead of failing immediately.
        for _ in 0..150 {
            if is_logged_in(home) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let _ = tokio::fs::remove_file(&launcher).await;
    }
    ensure!(
        is_logged_in(home),
        "DUCX login is required; complete the login in the DUCX window and save again"
    );
    Ok(())
}

fn is_logged_in(home: &Path) -> bool {
    let login_dir = home.join(".comate/login-user");
    fs::read_dir(&login_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        })
        .unwrap_or(false)
}

fn windows_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Locate Windows Terminal (wt.exe), which renders the login QR far more
/// reliably than the legacy console. Returns None when it is not installed.
fn windows_terminal() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        candidates.push(local.join("Microsoft/WindowsApps/wt.exe"));
    }
    if let Ok(output) = std::process::Command::new("where.exe")
        .arg("wt.exe")
        .output()
        && output.status.success()
    {
        candidates.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}
