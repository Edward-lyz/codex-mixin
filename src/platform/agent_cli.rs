//! Locating and driving coding-agent CLIs on each OS: `PATH` lookup with the
//! native launcher names, real-path resolution, bounded command runs,
//! desktop-app version reading, and the cross-process update lock.
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

/// Desktop apps whose installed version the Agents page shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopApp {
    /// The Codex desktop app, `ChatGPT.app` on macOS.
    Codex,
    /// The Claude desktop app.
    ClaudeDesktop,
    /// The DeepSeek Harness desktop app.
    Dsh,
}

/// What a finished command printed.
#[derive(Debug)]
pub struct CommandOutcome {
    /// Standard output alone, for callers that parse it as data.
    pub stdout: String,
    /// Standard output and standard error, without ANSI colour, for version
    /// probes and failure messages.
    pub text: String,
}

/// Where a command name is on `PATH`, native launcher names included.
pub fn find_command(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        for candidate in super::path_executable_candidates(&directory, name) {
            if candidate.is_file() {
                return Some(candidate);
            }
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

/// Install locations of a desktop app, most preferred first.
pub fn desktop_app_candidates(app: DesktopApp) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos_app_candidates(app)
    }
    #[cfg(windows)]
    {
        windows_app_candidates(app)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = app;
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn macos_app_candidates(app: DesktopApp) -> Vec<PathBuf> {
    let bundles: &[&str] = match app {
        DesktopApp::Codex => &["/Applications/ChatGPT.app", "/Applications/Codex.app"],
        DesktopApp::ClaudeDesktop => &["/Applications/Claude.app"],
        DesktopApp::Dsh => &["/Applications/DeepSeek Harness.app"],
    };
    let mut candidates: Vec<PathBuf> = bundles
        .iter()
        .map(|bundle| PathBuf::from(*bundle))
        .collect();
    if let Ok(home) = super::home_dir_required() {
        candidates.extend(
            bundles
                .iter()
                .map(|bundle| home.join(bundle.trim_start_matches('/'))),
        );
    }
    candidates
}

#[cfg(windows)]
fn windows_app_candidates(app: DesktopApp) -> Vec<PathBuf> {
    let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    let local = PathBuf::from(local);
    match app {
        DesktopApp::Codex => vec![
            local.join("Programs").join("ChatGPT"),
            local.join("Programs").join("OpenAI").join("Codex"),
        ],
        DesktopApp::ClaudeDesktop => vec![local.join("AnthropicClaude")],
        DesktopApp::Dsh => vec![local.join("Programs").join("DeepSeek Harness")],
    }
}

/// Version strings of the desktop app installed at `location`: the bundle's
/// `CFBundleShortVersionString` on macOS, the Squirrel `app-<version>`
/// folders on Windows. Empty when the app is absent or names no version.
pub fn desktop_app_versions(location: &Path) -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        macos_bundle_version(location).into_iter().collect()
    }
    #[cfg(windows)]
    {
        let versions = squirrel_app_versions(location);
        if !versions.is_empty() {
            return versions;
        }
        let executable = location.join("DeepSeek Harness.exe");
        if !executable.is_file() {
            return Vec::new();
        }
        // DSH's Electron installer does not use Squirrel's app-version folders.
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "(Get-Item -LiteralPath $env:CODEX_MIXIN_APP_EXE -ErrorAction Stop).VersionInfo.ProductVersion"])
            .env("CODEX_MIXIN_APP_EXE", &executable);
        run_command(command, Duration::from_secs(5), "read DSH app version")
            .ok()
            .map(|outcome| outcome.stdout.trim().to_owned())
            .filter(|version| !version.is_empty())
            .into_iter()
            .collect()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = location;
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn macos_bundle_version(bundle: &Path) -> Option<String> {
    let plist = bundle.join("Contents").join("Info.plist");
    let bytes = std::fs::read(&plist).ok()?;
    if !bytes.starts_with(b"bplist") {
        return plist_key_string(
            &String::from_utf8_lossy(&bytes),
            "CFBundleShortVersionString",
        );
    }
    // A binary property list needs the system tool to read one key.
    let mut command = Command::new("/usr/bin/plutil");
    command
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(&plist);
    let outcome = run_command(command, Duration::from_secs(5), "read the app's version").ok()?;
    let version = outcome.stdout.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// The `<string>` value of `key` in an XML property list, when present.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn plist_key_string(plist: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{key}</key>");
    let rest = &plist[plist.find(&marker)? + marker.len()..];
    let open = rest.find("<string>")? + "<string>".len();
    let close = rest[open..].find("</string>")? + open;
    let value = rest[open..close].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Versions named by Squirrel's `app-<version>` folders under `root`.
#[cfg_attr(not(windows), allow(dead_code))]
fn squirrel_app_versions(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(version) = name.to_str().and_then(|name| name.strip_prefix("app-")) else {
            continue;
        };
        if !version.is_empty() && entry.path().is_dir() {
            versions.push(version.to_owned());
        }
    }
    versions
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn dsh_desktop_bundle_is_detected_without_a_cli() {
        let candidates = desktop_app_candidates(DesktopApp::Dsh);
        assert!(candidates.contains(&PathBuf::from("/Applications/DeepSeek Harness.app")));
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("DeepSeek Harness.app");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        std::fs::write(bundle.join("Contents/Info.plist"), "<plist><dict><key>CFBundleShortVersionString</key><string>0.2.1-rc.2</string></dict></plist>").unwrap();
        assert_eq!(desktop_app_versions(&bundle), vec!["0.2.1-rc.2"]);
    }

    #[test]
    fn last_lines_keeps_the_tail_without_colour() {
        let text = "\u{1b}[31mfirst\u{1b}[0m\n\nsecond\nthird\nfourth\n";
        assert_eq!(last_lines(text, 2), "third · fourth");
        assert_eq!(last_lines("\n \n", 2), "");
    }

    #[test]
    fn plist_key_string_reads_the_value() {
        let plist = r#"<plist><dict>
            <key>CFBundleName</key><string>Claude</string>
            <key>CFBundleShortVersionString</key>
            <string>0.11.6</string>
        </dict></plist>"#;
        assert_eq!(
            plist_key_string(plist, "CFBundleShortVersionString").as_deref(),
            Some("0.11.6")
        );
        assert_eq!(plist_key_string(plist, "CFBundleVersion"), None);
        assert_eq!(
            plist_key_string("<plist/>", "CFBundleShortVersionString"),
            None
        );
    }

    #[test]
    fn squirrel_app_versions_reads_app_folders() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("app-0.11.6")).unwrap();
        std::fs::create_dir_all(directory.path().join("app-0.12.0")).unwrap();
        std::fs::create_dir_all(directory.path().join("other")).unwrap();
        std::fs::write(directory.path().join("app-9.9.9"), b"").unwrap();
        let mut versions = squirrel_app_versions(directory.path());
        versions.sort();
        assert_eq!(versions, vec!["0.11.6".to_owned(), "0.12.0".to_owned()]);
        assert!(squirrel_app_versions(&directory.path().join("absent")).is_empty());
    }

    #[test]
    fn agent_update_lock_is_exclusive_within_a_process() {
        let directory = tempfile::tempdir().unwrap();
        let first = AgentUpdateLock::try_acquire_in(directory.path(), "codex")
            .unwrap()
            .expect("first lock");
        assert!(
            AgentUpdateLock::try_acquire_in(directory.path(), "codex")
                .unwrap()
                .is_none(),
            "a second process must not take the same agent"
        );
        assert!(
            AgentUpdateLock::try_acquire_in(directory.path(), "claude")
                .unwrap()
                .is_some(),
            "another agent is a different lock"
        );
        drop(first);
        assert!(
            AgentUpdateLock::try_acquire_in(directory.path(), "codex")
                .unwrap()
                .is_some(),
            "the lock is free after release"
        );
    }

    #[test]
    fn agent_update_lock_rejects_unknown_ids() {
        let directory = tempfile::tempdir().unwrap();
        assert!(AgentUpdateLock::try_acquire_in(directory.path(), "../codex").is_err());
        assert!(AgentUpdateLock::try_acquire_in(directory.path(), "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_reports_the_exit_status_and_output_tail() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo boom >&2; exit 3"]);
        let error = run_command(command, Duration::from_secs(10), "sh test").unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("boom"), "{message}");
        assert!(message.contains('3'), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn run_command_kills_a_child_that_times_out() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let error = run_command(command, Duration::from_millis(300), "sh test").unwrap_err();
        assert!(format!("{error:#}").contains("timed out"), "{error:#}");
    }

    #[cfg(windows)]
    #[test]
    fn run_command_reports_the_exit_status_and_output_tail() {
        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/c", "echo boom 1>&2 & exit /b 3"]);
        let error = run_command(command, Duration::from_secs(10), "cmd test").unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("boom"), "{message}");
        assert!(message.contains('3'), "{message}");
    }
}
