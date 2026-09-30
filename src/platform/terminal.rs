//! Visible terminal windows for interactive steps (QR-code logins) requested
//! by a process that has no console of its own, such as a GUI-launched CLI.

use std::path::{Path, PathBuf};

use anyhow::Context;

/// An interactive command to show in a new terminal window.
pub struct TerminalCommand<'a> {
    pub title: &'a str,
    /// Directory for the generated launcher script.
    pub script_directory: &'a Path,
    /// Isolated home directory the command runs with, if any.
    pub home: Option<&'a Path>,
    pub environment: &'a [(&'a str, &'a Path)],
    pub program: &'a Path,
    pub arguments: &'a [&'a str],
}

/// Open a new terminal window running the command and return without waiting;
/// callers observe the command's effect instead of its exit status. Returns
/// the launcher script path, which callers delete once the step completes.
pub fn open_terminal_window(command: &TerminalCommand<'_>) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(command.script_directory)
        .with_context(|| format!("create {}", command.script_directory.display()))?;
    implementation::open(command)
}

fn environment<'a>(command: &TerminalCommand<'a>) -> Vec<(&'a str, &'a Path)> {
    let home = command
        .home
        .into_iter()
        .flat_map(|home| super::paths::HOME_VARIABLES.map(|variable| (variable, home)));
    home.chain(command.environment.iter().copied()).collect()
}

#[cfg(unix)]
fn quoted(value: &str) -> String {
    super::shell_quote(value)
}

#[cfg(any(windows, test))]
fn windows_script(command: &TerminalCommand<'_>) -> anyhow::Result<String> {
    use super::shell::powershell_quote;
    // Windows PowerShell 5 reads UTF-8 scripts correctly only with a BOM.
    let mut script = String::from(
        "\u{feff}$ErrorActionPreference = 'Stop'\r\n[Console]::OutputEncoding = [Text.Encoding]::UTF8\r\n",
    );
    for (key, value) in environment(command) {
        anyhow::ensure!(
            !key.is_empty()
                && key
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_'),
            "invalid terminal environment variable: {key}"
        );
        script.push_str(&format!(
            "$env:{key} = {}\r\n",
            powershell_quote(&value.to_string_lossy())
        ));
    }
    script.push_str(&format!(
        "& {}",
        powershell_quote(&command.program.to_string_lossy())
    ));
    for argument in command.arguments {
        script.push(' ');
        script.push_str(&powershell_quote(argument));
    }
    script.push_str("\r\nexit $LASTEXITCODE\r\n");
    Ok(script)
}

#[cfg(windows)]
mod implementation {
    use super::*;

    pub(super) fn open(command: &TerminalCommand<'_>) -> anyhow::Result<PathBuf> {
        // DUCX draws the login QR with Unicode half-block glyphs. The legacy
        // console renders those with the wrong cell aspect ratio, so prefer
        // Windows Terminal. The launcher switches to UTF-8 first; a script
        // file avoids fragile nested-quote parsing, and exporting variables
        // inside it matters because wt.exe hands the command to an already
        // running host that does not inherit the wt.exe environment.
        let launcher = command.script_directory.join("codex-mixin-terminal.ps1");
        std::fs::write(&launcher, windows_script(command)?).context("write terminal launcher")?;
        if let Some(terminal) = windows_terminal() {
            // `-w new` opens a new foreground window instead of a tab in a
            // possibly minimized host, so the user notices it.
            std::process::Command::new(terminal)
                .args([
                    "-w",
                    "new",
                    "--title",
                    command.title,
                    "powershell.exe",
                    "-NoLogo",
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(&launcher)
                .spawn()
                .context("open Windows Terminal")?;
        } else {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
            std::process::Command::new("powershell.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(&launcher)
                .creation_flags(CREATE_NEW_CONSOLE)
                .spawn()
                .context("open a console window")?;
        }
        Ok(launcher)
    }

    /// Locate Windows Terminal (wt.exe). Returns None when it is not installed.
    fn windows_terminal() -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            candidates.push(local.join("Microsoft/WindowsApps/wt.exe"));
        }
        let mut lookup = std::process::Command::new("where.exe");
        lookup.arg("wt.exe");
        crate::platform::prepare_background_command(&mut lookup);
        if let Ok(output) = lookup.output()
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
}

#[cfg(test)]
mod windows_script_tests {
    use super::*;

    #[test]
    fn windows_launcher_quotes_paths() {
        let script = windows_script(&TerminalCommand {
            title: "Login",
            script_directory: Path::new("/tmp"),
            home: Some(Path::new("C:/O'Neil/$HOME/%PATH%/home")),
            environment: &[],
            program: Path::new("C:/O'Neil/$HOME/%PATH%/ducx.exe"),
            arguments: &["a b", "$(exit 7)"],
        })
        .unwrap();
        assert!(script.starts_with('\u{feff}'));
        assert!(script.contains("$env:HOME = 'C:/O''Neil/$HOME/%PATH%/home'"));
        assert!(script.contains("& 'C:/O''Neil/$HOME/%PATH%/ducx.exe' 'a b' '$(exit 7)'"));
    }
}

#[cfg(unix)]
mod implementation {
    use super::*;

    fn posix_script(command: &TerminalCommand<'_>) -> String {
        let mut script = format!(
            "#!/bin/sh\nprintf '\\033]0;%s\\007' {}\n",
            quoted(command.title)
        );
        for (key, value) in environment(command) {
            script.push_str(&format!(
                "export {key}={}\n",
                quoted(&value.to_string_lossy())
            ));
        }
        script.push_str(&quoted(&command.program.to_string_lossy()));
        for argument in command.arguments {
            script.push(' ');
            script.push_str(&quoted(argument));
        }
        script.push_str("\nstatus=$?\necho\necho \"Finished (exit $status). You can close this window.\"\nexit $status\n");
        script
    }

    fn write_script(command: &TerminalCommand<'_>, name: &str) -> anyhow::Result<PathBuf> {
        let launcher = command.script_directory.join(name);
        std::fs::write(&launcher, posix_script(command)).context("write terminal launcher")?;
        crate::platform::make_private_executable(&launcher)
            .context("make terminal launcher executable")?;
        Ok(launcher)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn open(command: &TerminalCommand<'_>) -> anyhow::Result<PathBuf> {
        // Terminal runs `.command` files in a new window.
        let launcher = write_script(command, "codex-mixin-terminal.command")?;
        let status = std::process::Command::new("/usr/bin/open")
            .args(["-a", "Terminal"])
            .arg(&launcher)
            .status()
            .context("open Terminal")?;
        anyhow::ensure!(
            status.success(),
            "Terminal could not open {}",
            launcher.display()
        );
        Ok(launcher)
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn open(command: &TerminalCommand<'_>) -> anyhow::Result<PathBuf> {
        let graphical =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        anyhow::ensure!(
            graphical,
            "no graphical session is available to show a terminal window; run this step in an interactive terminal"
        );
        let launcher = write_script(command, "codex-mixin-terminal.sh")?;
        std::process::Command::new("x-terminal-emulator")
            .arg("-T")
            .arg(command.title)
            .arg("-e")
            .arg(&launcher)
            .spawn()
            .context(
                "open x-terminal-emulator; run this step in an interactive terminal instead",
            )?;
        Ok(launcher)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn launcher_exports_the_environment_and_quotes_arguments() {
            let script = posix_script(&TerminalCommand {
                title: "Login",
                script_directory: Path::new("/tmp"),
                home: Some(Path::new("/tmp/a b")),
                environment: &[],
                program: Path::new("/opt/it's/ducx"),
                arguments: &["login"],
            });
            assert!(script.contains("export HOME='/tmp/a b'\n"));
            assert!(script.contains("'/opt/it'\\''s/ducx' 'login'\n"));
        }
    }
}
