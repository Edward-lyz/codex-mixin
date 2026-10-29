//! Native shell startup entries. Callers own the undo journal and serialize writes.
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Select user profiles without replacing vendor executables or changing PATH.
/// The returned scripts contain no credentials. Blocking on Windows profile discovery.
pub(crate) fn profiles(
    id: &str,
    executable: &Path,
    config: &Path,
) -> anyhow::Result<Vec<(PathBuf, String)>> {
    let command = match id {
        "agy" | "muse" | "fx" | "empryo" | "ante" => id,
        "cursor-local" => "cursor",
        "commandcode" => "command-code",
        _ => anyhow::bail!("agent does not require a startup entry"),
    };
    anyhow::ensure!(
        executable.is_absolute() && config.is_absolute(),
        "startup paths must be absolute"
    );
    anyhow::ensure!(
        !executable
            .to_string_lossy()
            .chars()
            .chain(config.to_string_lossy().chars())
            .any(char::is_control),
        "invalid startup path"
    );
    let home = super::home_dir_required()?;
    if cfg!(windows) {
        let mut query = std::process::Command::new("powershell.exe");
        query.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8; [Environment]::GetFolderPath('MyDocuments')"]);
        super::prepare_background_command(&mut query);
        let result = super::agent_cli::run_command(
            query,
            Duration::from_secs(5),
            "locate PowerShell profiles",
        )?;
        let documents = PathBuf::from(result.stdout.trim());
        anyhow::ensure!(
            documents.is_absolute(),
            "PowerShell documents directory is unavailable"
        );
        return Ok(["PowerShell", "WindowsPowerShell"]
            .into_iter()
            .map(|directory| {
                (
                    documents.join(directory).join("profile.ps1"),
                    script(id, command, executable, config, "powershell"),
                )
            })
            .collect());
    }
    let zsh = std::env::var_os("ZDOTDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone());
    let config_root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    anyhow::ensure!(
        zsh.is_absolute() && config_root.is_absolute(),
        "shell configuration roots must be absolute"
    );
    let login = [".bash_profile", ".bash_login", ".profile"]
        .into_iter()
        .map(|name| home.join(name))
        .find(|path| path.exists())
        .unwrap_or_else(|| home.join(".profile"));
    let posix = script(id, command, executable, config, "posix");
    Ok(vec![
        (zsh.join(".zshrc"), posix.clone()),
        (home.join(".bashrc"), posix.clone()),
        (login, posix),
        (
            config_root
                .join("fish/conf.d")
                .join(format!("codex-mixin-{id}.fish")),
            script(id, command, executable, config, "fish"),
        ),
    ])
}

fn script(id: &str, command: &str, executable: &Path, config: &Path, shell: &str) -> String {
    let path = executable.to_string_lossy();
    let config = config.to_string_lossy();
    let body = match shell {
        "powershell" => format!(
            "function global:{command} {{ $previous = [Environment]::GetEnvironmentVariable('CODEX_GATEWAY_CONFIG', 'Process'); try {{ $env:CODEX_GATEWAY_CONFIG = {}; & {} --no-tui agent run {id} -- @args }} finally {{ if ($null -eq $previous) {{ Remove-Item Env:CODEX_GATEWAY_CONFIG -ErrorAction SilentlyContinue }} else {{ $env:CODEX_GATEWAY_CONFIG = $previous }} }} }}",
            super::shell::powershell_quote(&config),
            super::shell::powershell_quote(&path)
        ),
        "fish" => format!(
            "function {command} --wraps {command}\n    env {} {} --no-tui agent run {id} -- $argv\nend",
            fish_quote(&format!("CODEX_GATEWAY_CONFIG={config}")),
            fish_quote(&path)
        ),
        _ => {
            let invocation = format!(
                "env {} {} --no-tui agent run {id} --",
                super::shell_quote(&format!("CODEX_GATEWAY_CONFIG={config}")),
                super::shell_quote(&path)
            );
            format!("alias {command}={}", super::shell_quote(&invocation))
        }
    };
    format!("# >>> codex-mixin managed {id} >>>\n{body}\n# <<< codex-mixin managed {id} <<<\n")
}

fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_scripts_quote_paths_and_forward_arguments_without_keys() {
        let path = Path::new("/Applications/O'Neil $HOME/Codex Mixin");
        let config = Path::new("/tmp/Mixin config.json");
        let posix = script("agy", "agy", path, config, "posix");
        assert!(posix.contains("agent run agy --"));
        assert!(!posix.contains("GEMINI_API_KEY"));
        let powershell = script("commandcode", "command-code", path, config, "powershell");
        assert!(powershell.contains("O''Neil $HOME"));
        assert!(powershell.contains("-- @args"));
        let fish = script("muse", "muse", path, config, "fish");
        assert!(fish.contains("O\\'Neil $HOME"));
        assert!(fish.contains("-- $argv"));
    }

    #[cfg(unix)]
    #[test]
    fn normal_terminal_command_forwards_config_arguments_and_exit_status() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let executable = root.join("O'Neil $HOME Mixin");
        let config = root.join("custom config.json");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$CODEX_GATEWAY_CONFIG\" \"$@\"\nexit 23\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let profile = root.join("profile");
        std::fs::write(
            &profile,
            script("agy", "agy", &executable, &config, "posix"),
        )
        .unwrap();
        let output = std::process::Command::new("bash")
            .args(["--noprofile", "--norc", "-ic"])
            .arg(format!(
                "source {}\nagy --prompt 'two words' '$literal'",
                super::super::shell_quote(&profile.to_string_lossy())
            ))
            .env("HOME", &root)
            .env("CODEX_GATEWAY_CONFIG", "unrelated-config")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(23));
        let lines = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            lines.lines().collect::<Vec<_>>(),
            vec![
                config.to_str().unwrap(),
                "--no-tui",
                "agent",
                "run",
                "agy",
                "--",
                "--prompt",
                "two words",
                "$literal"
            ]
        );
    }
}
