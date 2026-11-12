//! OAuth credential locations and native browser/credential-store adapters.
use anyhow::Context;
use std::path::{Path, PathBuf};

pub fn oauth_path(provider: &str) -> PathBuf {
    let config = std::env::var_os("CODEX_GATEWAY_CONFIG")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join(".codex-mixin/config.json"));
    config
        .with_file_name("oauth")
        .join(format!("{provider}.json"))
}

pub fn codex_auth_path() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join(".codex"))
        .join("auth.json")
}

pub fn official_auth_path() -> PathBuf {
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

pub fn agy_external_token_paths() -> [PathBuf; 2] {
    [
        super::home_dir().join(".gemini/jetski-standalone-oauth-token"),
        super::home_dir().join(".gemini/antigravity-cli/antigravity-oauth-token"),
    ]
}

/// Files written by an interactive `claude /login`: the credential file
/// (Linux/Windows; macOS keeps the secret in the Keychain) and the global
/// state file that records the signed-in account on every OS.
pub fn claude_code_login_paths() -> [PathBuf; 2] {
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let credentials = config_dir
        .clone()
        .unwrap_or_else(|| super::home_dir().join(".claude"))
        .join(".credentials.json");
    let state = config_dir
        .map(|dir| dir.join(".claude.json"))
        .unwrap_or_else(|| super::home_dir().join(".claude.json"));
    [credentials, state]
}

pub fn kiro_auth_path() -> PathBuf {
    std::env::var_os("CODEX_MIXIN_KIRO_AUTH_FILE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join(".aws/sso/cache/kiro-auth-token.json"))
}

pub fn kiro_cli_auth_path() -> Option<PathBuf> {
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

pub fn refresh_kiro_cli() -> anyhow::Result<()> {
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

pub fn agy_settings_path() -> PathBuf {
    super::home_dir().join(".gemini/antigravity-cli/settings.json")
}

pub fn google_adc_path() -> PathBuf {
    std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            super::home_dir().join(".config/gcloud/application_default_credentials.json")
        })
}

pub fn agy_auth_path() -> Option<PathBuf> {
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

pub fn save_oauth(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
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

pub fn lock_oauth(credentials: &Path) -> anyhow::Result<std::fs::File> {
    lock_oauth_within(credentials, std::time::Duration::from_secs(20))
}

fn lock_oauth_within(
    credentials: &Path,
    wait: std::time::Duration,
) -> anyhow::Result<std::fs::File> {
    use fs2::FileExt;
    use std::time::{Duration, Instant};
    let path = credentials.with_extension("lock");
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
    let deadline = Instant::now() + wait;
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

pub fn open_oauth_url(url: &str) -> anyhow::Result<()> {
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

pub fn codex_keyring(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
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

/// Desktop launchers do not inherit shell exports. Read the user's login
/// environment through their native shell, with bounded private stdout.
/// The result, including absence or failure, is shared for a minute across
/// processes, so short-lived CLI calls start at most one shell between them.
pub fn claude_code_setup_token() -> Option<String> {
    const NAME: &str = "CLAUDE_CODE_OAUTH_TOKEN";
    if let Ok(token) = std::env::var(NAME)
        && !token.trim().is_empty()
    {
        return Some(token);
    }
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    type CachedToken = Option<(Instant, Option<String>)>;
    static CACHE: OnceLock<Mutex<CachedToken>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(|| Mutex::new(None)).lock().ok()?;
    if let Some((at, token)) = &*cache
        && at.elapsed() < Duration::from_secs(30)
    {
        return token.clone();
    }
    #[cfg(unix)]
    let found = {
        let shell = std::env::var_os("SHELL")
            .map(PathBuf::from)
            .or_else(|| {
                Some(PathBuf::from(if cfg!(target_os = "macos") {
                    "/bin/zsh"
                } else {
                    "/bin/sh"
                }))
            })
            .filter(|path| path.is_absolute() && path.is_file());
        shell.and_then(|shell| {
            use std::os::unix::process::CommandExt;
            use std::time::{SystemTime, UNIX_EPOCH};
            let shared = oauth_path("claude-setup-token-cache");
            let shell_key = shell.to_string_lossy().into_owned();
            let now_ms = || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_millis() as u64)
            };
            let read_shared = || {
                let document: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&shared).ok()?).ok()?;
                let age = now_ms().checked_sub(document["checked_at_ms"].as_u64()?)?;
                (age < 60_000 && document["shell"].as_str() == Some(shell_key.as_str()))
                    .then(|| document["token"].as_str().map(str::to_owned))
            };
            if let Some(token) = read_shared() {
                return token;
            }
            // The holder finishes within the 3 s shell budget; recheck after it.
            let _lock = lock_oauth_within(&shared, Duration::from_secs(5)).ok()?;
            if let Some(token) = read_shared() {
                return token;
            }
            let probe = || {
                let mut command = std::process::Command::new(&shell);
                // An interactive shell touches the controlling terminal and is
                // stopped by SIGTTIN/SIGTTOU when it is not the foreground group.
                // Its own process group keeps that stop from freezing this process.
                command.process_group(0);
                command.args([
                    "-lic",
                    r#"printf '\036MIXIN_CLAUDE_TOKEN=%s\037' "$CLAUDE_CODE_OAUTH_TOKEN""#,
                ]);
                // Private-input mode suppresses stdout/stderr in failure errors.
                let output = super::agent_cli::run_command_with_input(
                    command,
                    Duration::from_secs(3),
                    "Claude login environment",
                    Some(Vec::new()),
                )
                .ok()?;
                let token = output
                    .stdout
                    .split('\u{001e}')
                    .find_map(|line| line.strip_prefix("MIXIN_CLAUDE_TOKEN="))?
                    .split('\u{001f}')
                    .next()?
                    .trim();
                (!token.is_empty()).then(|| token.to_owned())
            };
            let token = probe();
            let record = serde_json::json!({
                "checked_at_ms": now_ms(),
                "shell": shell_key,
                "token": token,
            });
            if let Ok(bytes) = serde_json::to_vec(&record) {
                let _ = save_oauth(&shared, &bytes);
            }
            token
        })
    };
    #[cfg(windows)]
    let found = windows_registry::CURRENT_USER
        .open("Environment")
        .ok()
        .and_then(|key| key.get_string(NAME).ok())
        .filter(|token| !token.trim().is_empty());
    #[cfg(not(any(unix, windows)))]
    let found = None;
    *cache = Some((Instant::now(), found.clone()));
    found
}
