//! OAuth credential locations and native browser/credential-store adapters.
use anyhow::Context;
use std::path::{Path, PathBuf};

pub(crate) fn oauth_path(provider: &str) -> PathBuf {
    let config = std::env::var_os("CODEX_GATEWAY_CONFIG")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join(".codex-mixin/config.json"));
    config
        .with_file_name("oauth")
        .join(format!("{provider}.json"))
}

pub(crate) fn codex_auth_path() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join(".codex"))
        .join("auth.json")
}

pub(crate) fn official_auth_path() -> PathBuf {
    let owned = oauth_path("official");
    if owned.is_file() {
        return owned;
    }
    codex_auth_path()
}

fn agy_auth_paths() -> Vec<PathBuf> {
    let mut paths = vec![oauth_path("agy")];
    paths.extend(agy_external_token_paths());
    paths
}

pub(crate) fn agy_external_token_paths() -> [PathBuf; 2] {
    [
        super::home_dir().join(".gemini/jetski-standalone-oauth-token"),
        super::home_dir().join(".gemini/antigravity-cli/antigravity-oauth-token"),
    ]
}

pub(crate) fn kiro_auth_path() -> PathBuf {
    std::env::var_os("CODEX_MIXIN_KIRO_AUTH_FILE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join(".aws/sso/cache/kiro-auth-token.json"))
}

pub(crate) fn kiro_cli_auth_path() -> Option<PathBuf> {
    if std::env::var_os("CODEX_MIXIN_KIRO_AUTH_FILE").is_some_and(|path| !path.is_empty()) {
        return None;
    }
    #[cfg(target_os = "macos")]
    let path = super::home_dir().join("Library/Application Support/kiro-cli/data.sqlite3");
    #[cfg(target_os = "windows")]
    let path = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join("AppData/Roaming"))
        .join("kiro-cli/data.sqlite3");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let path = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join(".local/share"))
        .join("kiro-cli/data.sqlite3");
    Some(path)
}

pub(crate) fn refresh_kiro_cli() -> anyhow::Result<()> {
    let executable = super::agent_cli::find_command("kiro-cli")
        .context("kiro-cli is required to refresh its own login")?;
    let mut command = std::process::Command::new(executable);
    command.args(["debug", "refresh-auth-token"]);
    super::agent_cli::run_command(
        command,
        std::time::Duration::from_secs(15),
        "refresh kiro-cli login",
    )
    .map(|_| ())
    .map_err(|_| anyhow::anyhow!("kiro-cli could not refresh its login; sign in again"))
}

pub(crate) fn agy_settings_path() -> PathBuf {
    super::home_dir().join(".gemini/antigravity-cli/settings.json")
}

pub(crate) fn google_adc_path() -> PathBuf {
    std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            super::home_dir().join(".config/gcloud/application_default_credentials.json")
        })
}

pub(crate) fn agy_auth_path() -> Option<PathBuf> {
    let mut paths = agy_auth_paths();
    let owned = paths.remove(0);
    if owned.is_file() {
        return Some(owned);
    }
    paths
        .into_iter()
        .filter(|path| path.is_file())
        .max_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
}

pub(crate) fn save_oauth(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .context("OAuth credentials have no parent directory")?;
    std::fs::create_dir_all(parent)?;
    super::restrict_owner_only_dir(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    super::set_owner_only_mode_on(staged.as_file())?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    super::persist_temp_path(staged.into_temp_path(), path)?;
    Ok(())
}

pub(crate) fn lock_oauth(provider: &str) -> anyhow::Result<std::fs::File> {
    use fs2::FileExt;
    use std::time::{Duration, Instant};
    let path = oauth_path(provider).with_extension("lock");
    let parent = path
        .parent()
        .context("OAuth lock has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    super::restrict_owner_only_dir(parent)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    super::set_owner_only_mode_on(&file)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(error) => return Err(error).context("lock OAuth token refresh"),
        }
    }
}

pub(crate) fn open_oauth_url(url: &str) -> anyhow::Result<()> {
    let parsed = url::Url::parse(url)?;
    anyhow::ensure!(
        parsed.scheme() == "https",
        "OAuth browser URL must use HTTPS"
    );
    #[cfg(target_os = "macos")]
    let command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let command = {
        let mut command = std::process::Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    super::agent_cli::run_command(
        command,
        std::time::Duration::from_secs(5),
        "open OAuth browser",
    )?;
    Ok(())
}

pub(crate) fn codex_keyring(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    if path != codex_auth_path() {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    {
        use std::io::Read;
        use std::process::{Command, Stdio};
        use wait_timeout::ChildExt;
        let parent = path.parent().context("Codex auth has no home directory")?;
        let canonical = parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf());
        let digest = ring::digest::digest(
            &ring::digest::SHA256,
            canonical.to_string_lossy().as_bytes(),
        );
        let key: String = digest.as_ref()[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut child = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "Codex Auth",
                "-a",
                &format!("cli|{key}"),
                "-w",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .context("credential-store stdout missing")?;
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let Some(status) = child.wait_timeout(std::time::Duration::from_secs(3))? else {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Codex credential-store lookup timed out");
        };
        if status.code() == Some(44) {
            let _ = reader.join();
            return Ok(None);
        }
        anyhow::ensure!(
            status.success(),
            "Codex credential-store lookup returned {status}"
        );
        let bytes = reader
            .join()
            .map_err(|_| anyhow::anyhow!("credential-store reader failed"))??;
        anyhow::ensure!(
            bytes.len() <= 1024 * 1024,
            "credential-store record exceeds 1 MiB"
        );
        Ok(Some(bytes))
    }
    #[cfg(not(target_os = "macos"))]
    Ok(None)
}
