//! Managed DUCX install + login, isolated under `~/.codex-mixin/ducx`.
//!
//! Tracks Baidu's `baidu-cx` package. DUCX is only used to mint the native
//! `comate_custom_header`; it never touches the user's own `~/.baidu-cx`
//! install, config, or hooks.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, ensure};
use codex_mixin::platform::{
    TerminalCommand, executable_file_name, home_dir_required, make_executable,
    open_terminal_window, package_target, set_owner_only_dir_mode, set_tokio_home_env,
};
use futures_util::StreamExt;
use reqwest::Client;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::ducx_session::is_logged_in;

const DOWNLOAD_BASE_URL: &str = "https://baidu-cc-client.bj.bcebos.com/baidu-cx";
const DOWNLOAD_ATTEMPTS: usize = 3;
const DOWNLOAD_PROGRESS_INTERVAL: u64 = 16 * 1024 * 1024;
const MINIMUM_ARCHIVE_SIZE: u64 = 20 * 1024 * 1024;
/// How long a login shown in a separate terminal window may take (9 minutes,
/// inside the 10-minute command budget of the GUI shells).
const WINDOW_LOGIN_POLL_INTERVAL: Duration = Duration::from_secs(2);
const WINDOW_LOGIN_POLL_ATTEMPTS: usize = 270;
/// Launchers shipped in the package `bin` directory, preferred first.
const PACKAGE_LAUNCHER_STEMS: [&str; 2] = ["baidu-codex", "codex"];
const BUNDLED_STATE_FILES: [&str; 4] = ["config.toml", "auth.json", "hooks.json", "user.json"];

/// Install the managed DUCX when missing and make sure it is signed in.
pub(super) async fn ensure_managed_ducx() -> anyhow::Result<PathBuf> {
    let ducx_root = home_dir_required()
        .context("a home directory is required to install managed DUCX")?
        .join(".codex-mixin/ducx");
    let install_home = ducx_root.join("home");
    let executable = install_home
        .join(".baidu-cx/baidu-cx/bin")
        .join(executable_file_name("ducx"));
    if !executable.is_file() {
        super::progress_step("Preparing DUCX authentication");
        install_package(&ducx_root, &install_home, &executable).await?;
    }
    ensure_logged_in(&executable, &install_home).await?;
    Ok(executable)
}

async fn install_package(
    ducx_root: &Path,
    install_home: &Path,
    executable: &Path,
) -> anyhow::Result<()> {
    let target = package_target().context("managed DUCX is not available on this system")?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .context("create DUCX download client")?;
    let version_url = format!("{DOWNLOAD_BASE_URL}/baidu_cx_latest_version.txt");
    let version = download_text(&client, &version_url, "DUCX version")
        .await?
        .trim()
        .to_owned();
    ensure!(
        !version.is_empty()
            && version
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || value == '.' || value == '-'),
        "DUCX version response is invalid"
    );
    let archive_name = format!("baidu-cx-{}-{}-{version}.tar.bz2", target.os, target.arch);
    let download_dir = ducx_root.join("downloads");
    tokio::fs::create_dir_all(&download_dir).await?;
    set_owner_only_dir_mode(&download_dir)?;
    let archive = download_dir.join(&archive_name);
    super::progress_step("Downloading DUCX authentication package");
    download_archive(
        &client,
        &format!("{DOWNLOAD_BASE_URL}/{archive_name}"),
        &archive,
    )
    .await?;
    ensure!(
        tokio::fs::metadata(&archive).await?.len() >= MINIMUM_ARCHIVE_SIZE,
        "DUCX archive is unexpectedly small: {}",
        archive.display()
    );

    let root = install_home.join(".baidu-cx");
    tokio::fs::create_dir_all(&root).await?;
    let staging = root.join(format!(".baidu-cx-staging-{}", std::process::id()));
    super::progress_step("Installing DUCX authentication package");
    let archive_path = archive.clone();
    let staging_dir = staging.clone();
    let active = root.join("baidu-cx");
    let result = tokio::task::spawn_blocking(move || {
        let result = unpack_and_activate(&archive_path, &staging_dir, &active);
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging_dir);
        }
        result
    })
    .await
    .context("join DUCX package installation")?;
    result?;
    ensure!(
        executable.is_file(),
        "DUCX installation is missing {}",
        executable.display()
    );
    let _ = tokio::fs::remove_file(&archive).await;
    Ok(())
}

/// Extract the package into a staging directory, create the managed `ducx`
/// launcher, and atomically replace the active installation.
fn unpack_and_activate(archive: &Path, staging: &Path, active: &Path) -> anyhow::Result<()> {
    if staging.exists() {
        fs::remove_dir_all(staging)?;
    }
    fs::create_dir_all(staging)?;
    let file = fs::File::open(archive)
        .with_context(|| format!("open DUCX archive {}", archive.display()))?;
    tar::Archive::new(bzip2::read::BzDecoder::new(file))
        .unpack(staging)
        .with_context(|| format!("extract DUCX archive to {}", staging.display()))?;
    let bin = staging.join("bin");
    let launcher = PACKAGE_LAUNCHER_STEMS
        .into_iter()
        .map(|stem| bin.join(executable_file_name(stem)))
        .find(|path| path.is_file())
        .with_context(|| format!("DUCX archive has no launcher in {}", bin.display()))?;
    let managed = bin.join(executable_file_name("ducx"));
    fs::copy(&launcher, &managed)
        .with_context(|| format!("create managed DUCX launcher {}", managed.display()))?;
    make_executable(&managed)?;
    // Drop any bundled config/auth/hooks so the isolated install starts clean.
    for name in BUNDLED_STATE_FILES {
        let _ = fs::remove_file(staging.join(name));
    }
    match fs::symlink_metadata(active) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(active)?,
        // Earlier releases activated a versioned directory through a link.
        Ok(_) => fs::remove_file(active)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::rename(staging, active).context("activate the managed DUCX installation")
}

async fn download_text(client: &Client, url: &str, label: &str) -> anyhow::Result<String> {
    let mut last_error = None;
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        let response = client
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);
        match response {
            Ok(response) => match response.text().await {
                Ok(body) => return Ok(body),
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
        if attempt < DOWNLOAD_ATTEMPTS {
            eprintln!("warning: {label} download attempt {attempt} failed; retrying");
            tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
        }
    }
    match last_error {
        Some(error) => Err(error)
            .with_context(|| format!("download {label} after {DOWNLOAD_ATTEMPTS} attempts")),
        None => anyhow::bail!("download {label}: no attempt was made"),
    }
}

async fn download_archive(client: &Client, url: &str, archive: &Path) -> anyhow::Result<()> {
    let file_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .context("DUCX archive path has no file name")?;
    let partial = archive.with_file_name(format!("{file_name}.part"));
    let mut last_error = None;
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        let _ = tokio::fs::remove_file(&partial).await;
        match download_archive_once(client, url, &partial).await {
            Ok(()) => {
                if archive.exists() {
                    tokio::fs::remove_file(archive).await?;
                }
                tokio::fs::rename(&partial, archive).await?;
                return Ok(());
            }
            Err(error) => last_error = Some(error),
        }
        let _ = tokio::fs::remove_file(&partial).await;
        if attempt < DOWNLOAD_ATTEMPTS {
            eprintln!("warning: DUCX archive download attempt {attempt} failed; retrying");
            tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
        }
    }
    match last_error {
        Some(error) => Err(error).context("download DUCX archive after retries"),
        None => anyhow::bail!("download DUCX archive: no attempt was made"),
    }
}

async fn download_archive_once(client: &Client, url: &str, partial: &Path) -> anyhow::Result<()> {
    let response = client
        .get(url)
        .send()
        .await
        .context("download DUCX archive")?
        .error_for_status()
        .context("DUCX archive request failed")?;
    let expected_bytes = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(partial)
        .await
        .with_context(|| format!("create DUCX archive download {}", partial.display()))?;
    let mut downloaded_bytes = 0_u64;
    let mut next_progress = DOWNLOAD_PROGRESS_INTERVAL;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read DUCX archive response")?;
        file.write_all(&chunk).await?;
        downloaded_bytes += chunk.len() as u64;
        if downloaded_bytes >= next_progress {
            let downloaded_mib = downloaded_bytes / (1024 * 1024);
            match expected_bytes.map(|bytes| bytes.div_ceil(1024 * 1024)) {
                Some(total_mib) => super::progress_step(&format!(
                    "Downloading DUCX {downloaded_mib}/{total_mib} MiB"
                )),
                None => super::progress_step(&format!("Downloading DUCX {downloaded_mib} MiB")),
            }
            next_progress = downloaded_bytes + DOWNLOAD_PROGRESS_INTERVAL;
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    ensure!(downloaded_bytes > 0, "DUCX archive is empty");
    if let Some(expected_bytes) = expected_bytes {
        ensure!(
            downloaded_bytes == expected_bytes,
            "DUCX archive is incomplete: downloaded {downloaded_bytes} of {expected_bytes} bytes"
        );
    }
    Ok(())
}

/// Sign DUCX in. An interactive terminal logs in inline; a console-less
/// caller (a GUI shell) gets the QR-code login in a new terminal window and
/// this call waits until the login state appears.
async fn ensure_logged_in(executable: &Path, home: &Path) -> anyhow::Result<()> {
    if is_logged_in(home) {
        return Ok(());
    }
    let codex_home = home.join(".baidu-cx");
    super::progress_step("Waiting for DUCX login");
    if std::io::stdin().is_terminal() {
        eprintln!("\nDUCX login is required. Complete QR-code login in this terminal.");
        let mut command = Command::new(executable);
        set_tokio_home_env(&mut command, home);
        let status = command
            .arg("login")
            .env("CODEX_HOME", &codex_home)
            .env("DISABLE_DUCX_CLI_UPDATE", "1")
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context("failed to start DUCX login")?;
        ensure!(status.success(), "DUCX login failed with {status}");
    } else {
        let update_guard = Path::new("1");
        let launcher = open_terminal_window(&TerminalCommand {
            title: "百度 DUCX 登录",
            script_directory: home,
            home: Some(home),
            environment: &[
                ("CODEX_HOME", &codex_home),
                ("DISABLE_DUCX_CLI_UPDATE", update_guard),
                ("DISABLE_BAIDU_CLAUDE_UPDATE", update_guard),
            ],
            program: executable,
            arguments: &["login"],
        })
        .context("DUCX login needs an interactive terminal")?;
        for _ in 0..WINDOW_LOGIN_POLL_ATTEMPTS {
            if is_logged_in(home) {
                break;
            }
            tokio::time::sleep(WINDOW_LOGIN_POLL_INTERVAL).await;
        }
        let _ = tokio::fs::remove_file(&launcher).await;
    }
    ensure!(
        is_logged_in(home),
        "DUCX login did not complete; finish the QR-code login and try again"
    );
    super::progress_step("DUCX authentication completed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive_with_launcher(directory: &Path, stem: &str) -> PathBuf {
        let archive_path = directory.join("package.tar.bz2");
        let encoder = bzip2::write::BzEncoder::new(
            fs::File::create(&archive_path).unwrap(),
            bzip2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        for (name, contents) in [
            (
                format!("bin/{}", executable_file_name(stem)),
                &b"launcher"[..],
            ),
            ("config.toml".to_owned(), &b"bundled"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, contents).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        archive_path
    }

    #[test]
    fn installs_the_package_launcher_as_managed_ducx_and_replaces_the_old_install() {
        let directory = tempfile::tempdir().unwrap();
        let archive = archive_with_launcher(directory.path(), "baidu-codex");
        let active = directory.path().join("baidu-cx");
        fs::create_dir_all(active.join("stale")).unwrap();

        unpack_and_activate(&archive, &directory.path().join("staging"), &active).unwrap();

        let managed = active.join("bin").join(executable_file_name("ducx"));
        assert_eq!(fs::read(&managed).unwrap(), b"launcher");
        assert!(!active.join("config.toml").exists());
        assert!(!active.join("stale").exists());
        assert!(!directory.path().join("staging").exists());
    }

    #[test]
    fn accepts_packages_that_only_ship_the_codex_launcher() {
        let directory = tempfile::tempdir().unwrap();
        let archive = archive_with_launcher(directory.path(), "codex");
        let active = directory.path().join("baidu-cx");

        unpack_and_activate(&archive, &directory.path().join("staging"), &active).unwrap();

        assert!(
            active
                .join("bin")
                .join(executable_file_name("ducx"))
                .is_file()
        );
    }
}
