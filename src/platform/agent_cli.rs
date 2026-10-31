//! Locating and driving coding-agent CLIs on each OS: executable lookup with
//! native launcher names, real-path resolution, bounded command runs and the
//! cross-process update lock. Desktop registration lives in `agent_apps`.
//!
//! Functions here touch the file system and start processes; async callers
//! use `spawn_blocking`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Context;
use wait_timeout::ChildExt;

/// Lines of command output kept in a failure message.
const FAILURE_TAIL_LINES: usize = 3;

/// Devin follows XDG on Unix and the native roaming configuration on Windows.
pub(crate) fn devin_settings_path() -> anyhow::Result<PathBuf> {
    let root = if cfg!(windows) {
        super::framework_app_root()?.join("devin")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| super::home_dir().join(".config"))
            .join("devin")
    };
    anyhow::ensure!(
        root.is_absolute(),
        "Devin configuration root must be absolute"
    );
    Ok(root.join("config.json"))
}

/// Zed uses ~/.config on macOS rather than Application Support.
pub(crate) fn zed_settings_path() -> anyhow::Result<PathBuf> {
    let root = if cfg!(windows) {
        super::framework_app_root()?
    } else if cfg!(target_os = "macos") {
        super::home_dir_required()?.join(".config")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or(super::home_dir_required()?.join(".config"))
    };
    anyhow::ensure!(
        root.is_absolute(),
        "Zed configuration root must be absolute"
    );
    Ok(root.join("zed/settings.json"))
}

/// A bounded Aside runtime call. The marker carries only a model selection;
/// credentials stay in Aside's private provider file.
pub(crate) fn aside_default_model(
    account: &str,
    selection: Option<&serde_json::Value>,
) -> anyhow::Result<serde_json::Value> {
    anyhow::ensure!(
        account.starts_with('u')
            && account.len() > 1
            && account[1..].bytes().all(|byte| byte.is_ascii_digit()),
        "invalid Aside account"
    );
    let program = find_command("aside").context("Aside executable is not installed")?;
    let mut command = super::launcher_command(&program);
    let mut expression = String::new();
    if let Some(selection) = selection {
        expression.push_str(&format!(
            "aside.settings.set('defaultModel',{});",
            serde_json::to_string(selection)?
        ));
    }
    expression.push_str("console.log('CODEX_MIXIN_ASIDE_STATE '+JSON.stringify(aside.settings.get('defaultModel')??null))");
    command.args(["repl", "--account", account, "--host", "local", &expression]);
    let outcome = run_command(command, Duration::from_secs(3), "Aside runtime").map_err(|_| {
        anyhow::anyhow!("Aside runtime is unavailable; configuration remains staged")
    })?;
    let line = outcome
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("CODEX_MIXIN_ASIDE_STATE "))
        .context("Aside returned no confirmed model selection")?;
    let actual: serde_json::Value =
        serde_json::from_str(line).context("invalid Aside model selection")?;
    if let Some(selection) = selection {
        anyhow::ensure!(
            actual == *selection,
            "Aside did not confirm the requested model selection"
        );
    }
    Ok(actual)
}

/// Open a private handoff page instead of putting the key-bearing Cindy URI
/// on a command line. Cindy performs its own final import confirmation.
pub(crate) fn open_cindy_import(page: &Path, uri: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        page.is_absolute()
            && uri.starts_with("cindy://provider/import?v=1&data=")
            && uri
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":/?=&_-".contains(&byte)),
        "invalid Cindy import handoff"
    );
    super::ensure_owner_only_dir(page.parent().context("Cindy handoff has no parent")?)?;
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Codex Mixin import</title><p>Import the gateway provider, then confirm in Cindy.</p><a href=\"{}\">Continue in Cindy</a>",
        uri.replace('&', "&amp;")
    );
    super::write_owner_only(page, html.as_bytes())?;
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        Command::new("xdg-open")
    };
    command.arg(page);
    run_command(command, Duration::from_secs(10), "Cindy import handoff")?;
    Ok(())
}

pub(crate) use super::agent_apps::desktop_app_installation;
pub use super::agent_apps::{DesktopApp, desktop_app_candidates, desktop_app_versions};

/// What a finished command printed.
#[derive(Debug)]
pub struct CommandOutcome {
    /// Standard output alone, for callers that parse it as data.
    pub stdout: String,
    /// Standard output and standard error, without ANSI colour, for version
    /// probes and failure messages.
    pub text: String,
}

/// Where a command name is on `PATH`, native launcher names included. Desktop
/// apps inherit a smaller PATH than terminal sessions, so also check the
/// platform's standard user/system CLI installation directories afterwards.
pub fn find_command(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return None;
    }
    let mut directories: Vec<_> = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .collect();
    if let Ok(home) = super::home_dir_required() {
        directories.extend([
            home.join(".local/bin"),
            home.join(".cargo/bin"),
            home.join(".bun/bin"),
        ]);
    }
    #[cfg(windows)]
    for variable in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            let root = PathBuf::from(root);
            directories.extend([root.join("npm"), root.join("Microsoft/WinGet/Links")]);
        }
    }
    #[cfg(target_os = "macos")]
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    #[cfg(target_os = "linux")]
    directories.extend([
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/snap/bin"),
    ]);
    for directory in directories {
        // A current-directory PATH entry must not make an untrusted project
        // binary eligible for background version/upgrade probes.
        if !directory.is_absolute() {
            continue;
        }
        for candidate in super::path_executable_candidates(&directory, name) {
            if !candidate.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if !std::fs::metadata(&candidate)
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
                {
                    continue;
                }
            }
            return Some(candidate);
        }
    }
    None
}

/// The file a launcher really runs, with symbolic links and Windows
/// junctions resolved; the input path when it cannot be resolved.
pub fn real_executable(path: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    strip_verbatim_prefix(resolved)
}

#[cfg(windows)]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    // canonicalize answers with a \\?\ verbatim path on Windows; pattern
    // matching and display want the plain form back.
    let text = path.to_string_lossy();
    if let Some(plain) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(plain);
    }
    path
}

#[cfg(not(windows))]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

/// Run a prepared command to completion within `timeout`. The command never
/// Locate the Cursor editor's real executable so its child receives the
/// managed environment even when a desktop launcher would reuse a process.
pub fn cursor_editor_program() -> anyhow::Result<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from(
            "/Applications/Cursor.app/Contents/MacOS/Cursor",
        ));
        candidates.push(
            super::home_dir_required()?.join("Applications/Cursor.app/Contents/MacOS/Cursor"),
        );
    } else if cfg!(windows) {
        for variable in ["LOCALAPPDATA", "ProgramFiles"] {
            if let Some(root) = std::env::var_os(variable) {
                let root = PathBuf::from(root);
                candidates.extend([
                    root.join("Programs/cursor/Cursor.exe"),
                    root.join("cursor/Cursor.exe"),
                ]);
            }
        }
    }
    if let Some(program) = find_command("cursor") {
        candidates.push(program);
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| anyhow::anyhow!("Cursor editor executable is not installed"))
}

/// Run a prepared command to completion within `timeout`. The command never
/// reads standard input. A failure keeps the exit status and the last output
/// lines; a timeout kills the child and reports itself.
pub fn run_command(
    command: Command,
    timeout: Duration,
    what: &str,
) -> anyhow::Result<CommandOutcome> {
    run_command_with_input(command, timeout, what, None)
}

/// Supply private native-tool input through a pipe, never through argv.
/// Errors deliberately omit tool output when input may contain credentials.
pub fn run_command_with_input(
    mut command: Command,
    timeout: Duration,
    what: &str,
    input: Option<Vec<u8>>,
) -> anyhow::Result<CommandOutcome> {
    let private_input = input.is_some();
    anyhow::ensure!(
        input.as_ref().is_none_or(|bytes| bytes.len() <= 64 * 1024),
        "native tool input is too large"
    );
    command
        .stdin(if private_input {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().with_context(|| format!("run {what}"))?;
    if let Some(input) = input {
        let pipe = child.stdin.take();
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let _ = std::io::Write::write_all(&mut pipe, &input);
            }
        });
    }
    // Read both pipes while the child runs: a child that prints more than a
    // pipe buffer would otherwise block until the timeout fires.
    let stdout_reader = std::thread::spawn({
        let pipe = child.stdout.take();
        move || read_pipe(pipe)
    });
    let stderr_reader = std::thread::spawn({
        let pipe = child.stderr.take();
        move || read_pipe(pipe)
    });
    let status = match child
        .wait_timeout(timeout)
        .with_context(|| format!("wait for {what}"))?
    {
        Some(status) => status,
        None => {
            // Do not join the readers here: a descendant that outlives the
            // killed child can hold the pipes open, and the timeout must
            // still return. The readers end when the pipes close.
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("{what} timed out after {}s", timeout.as_secs());
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let mut combined = stdout.clone();
    if !stderr.trim().is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    let text = strip_ansi(&combined);
    if !status.success() {
        if private_input {
            anyhow::bail!("{what} failed with {status}");
        }
        let tail = last_lines(&text, FAILURE_TAIL_LINES);
        return Err(if tail.is_empty() {
            anyhow::anyhow!("{what} failed with {status}")
        } else {
            anyhow::anyhow!("{what} failed with {status}: {tail}")
        });
    }
    Ok(CommandOutcome { stdout, text })
}

/// What `<program> --version` printed, for callers that parse a version out
/// of it.
pub fn probe_version_output(program: &Path, timeout: Duration) -> anyhow::Result<String> {
    let mut command = super::launcher_command(program);
    command.arg("--version");
    Ok(run_command(
        command,
        timeout,
        &format!("{} --version", program.display()),
    )?
    .text)
}

fn read_pipe(pipe: Option<impl Read>) -> String {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(length) => {
                    let keep = length.min((1usize << 20).saturating_sub(bytes.len()));
                    bytes.extend_from_slice(&chunk[..keep]);
                }
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The last `count` non-empty lines of `text`, without colour, joined by
/// ` · `.
fn last_lines(text: &str, count: usize) -> String {
    let plain = strip_ansi(text);
    let mut kept: Vec<&str> = Vec::new();
    for line in plain.lines().rev() {
        if kept.len() == count {
            break;
        }
        let line = line.trim();
        if !line.is_empty() {
            kept.push(line);
        }
    }
    kept.reverse();
    kept.join(" · ")
}

/// Drop ANSI escape sequences so error text stays readable in the UI.
fn strip_ansi(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            output.push(character);
            continue;
        }
        // CSI: ESC [ … one final byte in @..~
        if characters.next() == Some('[') {
            for next in characters.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    output
}

/// A global npm install: the prefix npm installed into and the npm launcher
/// that owns it.
#[derive(Clone, Debug)]
pub struct NpmGlobalInstall {
    pub prefix: PathBuf,
    pub npm: PathBuf,
}

/// The npm launcher and prefix of a global `node_modules` directory, when
/// the directory is one: `<prefix>/lib/node_modules` on Unix,
/// `<prefix>/node_modules` on Windows. The launcher is the npm inside the
/// prefix when present, so an update cannot land in another npm's tree.
pub fn npm_global_install(modules: &Path) -> Option<NpmGlobalInstall> {
    let parent = modules.parent()?;
    let prefix = if cfg!(windows) {
        parent.to_path_buf()
    } else {
        // …/lib/node_modules, where … is the prefix
        if parent.file_name()?.to_str()? != "lib" {
            return None;
        }
        parent.parent()?.to_path_buf()
    };
    let local_npm = if cfg!(windows) {
        prefix.join("npm.cmd")
    } else {
        prefix.join("bin").join("npm")
    };
    let npm = if local_npm.is_file() {
        local_npm
    } else {
        find_command("npm")?
    };
    Some(NpmGlobalInstall { prefix, npm })
}

/// Whether this user can create files in `directory` right now, probed with
/// a temporary file so read-only mounts and Windows ACLs are both covered.
pub fn directory_writable(directory: &Path) -> bool {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".codex-mixin-write-test");
    match builder.tempfile_in(directory) {
        Ok(file) => {
            drop(file);
            true
        }
        Err(_) => false,
    }
}

/// Whether `real` is the CLI launcher a vendor installer put directly in
/// `~/.local/bin` (Windows keeps `claude.exe` there instead of a link into a
/// versioned folder).
pub fn is_local_bin_launcher(real: &Path, stem: &str) -> bool {
    let Ok(home) = super::home_dir_required() else {
        return false;
    };
    let directory = home.join(".local").join("bin");
    let Some(parent) = real.parent() else {
        return false;
    };
    if !same_path_text(parent, &directory) {
        return false;
    }
    real.file_name().and_then(|name| name.to_str())
        == Some(super::executable_file_name(stem).as_str())
}

fn same_path_text(left: &Path, right: &Path) -> bool {
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

/// Cross-process lock held while one agent's CLI is updated, so the CLI and
/// the menu app never upgrade the same agent at once.
pub struct AgentUpdateLock {
    file: std::fs::File,
}

impl AgentUpdateLock {
    /// Take the lock for `agent_id` under the user's state directory, or
    /// `None` while another process holds it.
    pub fn try_acquire(agent_id: &str) -> anyhow::Result<Option<Self>> {
        let directory = super::home_dir_required()?
            .join(".codex-mixin")
            .join("agent-updates");
        Self::try_acquire_in(&directory, agent_id)
    }

    fn try_acquire_in(directory: &Path, agent_id: &str) -> anyhow::Result<Option<Self>> {
        anyhow::ensure!(
            !agent_id.is_empty()
                && agent_id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()
                        || character == '-'
                        || character == '_'),
            "invalid agent id for the update lock: {agent_id}"
        );
        std::fs::create_dir_all(directory)
            .with_context(|| format!("create {}", directory.display()))?;
        let path = directory.join(format!("{agent_id}.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(Self { file })),
            // Windows reports contention as ERROR_LOCK_VIOLATION, not WouldBlock.
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                Ok(None)
            }
            Err(error) => Err(error).with_context(|| format!("lock {}", path.display())),
        }
    }
}

impl Drop for AgentUpdateLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
