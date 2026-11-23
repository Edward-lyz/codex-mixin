//! Desktop sign-in preferences. Gateway startup is owned by the CLI.

#[cfg(windows)]
const SHELL_SETTINGS: &str = r"Software\CodexMixin";
#[cfg(windows)]
const LIGHTWEIGHT: &str = "LightweightMode";

#[cfg(windows)]
pub fn skip_launch(background: bool) -> bool {
    use windows_registry::CURRENT_USER;

    let lightweight = CURRENT_USER
        .open(SHELL_SETTINGS)
        .and_then(|key| key.get_u32(LIGHTWEIGHT))
        .is_ok_and(|value| value != 0);
    if lightweight
        && !background
        && let Err(error) = CURRENT_USER
            .options()
            .write()
            .open(SHELL_SETTINGS)
            .and_then(|key| key.remove_value(LIGHTWEIGHT))
    {
        eprintln!("cannot clear desktop lightweight preference: {error}");
    }
    background && lightweight
}

#[cfg(target_os = "macos")]
pub(crate) use macos::{
    remember_gateway_login_choice, should_enable_gateway_at_login, skip_launch,
};

#[cfg(not(target_os = "macos"))]
pub fn should_enable_gateway_at_login() -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(not(target_os = "macos"))]
pub fn remember_gateway_login_choice() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn skip_launch(_: bool) -> bool {
    false
}

pub fn enter_lightweight() -> anyhow::Result<()> {
    #[cfg(windows)]
    windows_registry::CURRENT_USER
        .create(SHELL_SETTINGS)?
        .set_u32(LIGHTWEIGHT, 1)?;
    #[cfg(target_os = "macos")]
    macos::enter_lightweight();
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn handoff_desktop_login() {
    std::thread::spawn(|| {
        if let Err(error) = macos::handoff_desktop_login() {
            eprintln!("cannot migrate the previous menu bar login item: {error:#}");
        }
    });
}

#[cfg(target_os = "macos")]
mod macos {
    use std::fs;
    use std::io::{ErrorKind, Write as _};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context as _, ensure};
    use objc2_foundation::{NSUserDefaults, ns_string};
    use serde_json::Value;

    const MENU_LABEL: &str = "local.codex-mixin.menu-launch";
    const LOGIN_MARKER: &str = "Library/Application Support/Codex Mixin/launch-at-login.applied";

    pub(crate) fn should_enable_gateway_at_login() -> anyhow::Result<bool> {
        let home = home()?;
        if installed_executable(&home)?.is_none() {
            return Ok(false);
        }
        // The SwiftUI app wrote this marker after setting its login defaults.
        // A legacy menu agent also indicates an earlier installation.
        if home.join(LOGIN_MARKER).try_exists()?
            || home
                .join("Library/LaunchAgents")
                .join(format!("{MENU_LABEL}.plist"))
                .try_exists()?
        {
            return Ok(false);
        }

        let uid = Command::new("/usr/bin/id").arg("-u").output()?;
        ensure!(uid.status.success(), "resolve macOS user ID");
        let uid = std::str::from_utf8(&uid.stdout)?.trim();
        let output = Command::new("/bin/launchctl")
            .args(["print-disabled", &format!("gui/{uid}")])
            .output()?;
        ensure!(
            output.status.success(),
            "inspect macOS startup preferences: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let disabled = String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            line.split_once("=>").is_some_and(|(label, state)| {
                let label = label.trim().trim_matches('"');
                (label == "local.codex-mixin.service" || label == MENU_LABEL)
                    && matches!(state.trim(), "true" | "disabled")
            })
        });
        Ok(!disabled)
    }

    pub(crate) fn remember_gateway_login_choice() -> anyhow::Result<()> {
        let home = home()?;
        if installed_executable(&home)?.is_some() {
            let marker = home.join(LOGIN_MARKER);
            fs::create_dir_all(
                marker
                    .parent()
                    .context("resolve login preferences directory")?,
            )?;
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(marker)?;
        }
        Ok(())
    }

    pub(crate) fn skip_launch(background: bool) -> bool {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = ns_string!("lightweightMode");
        if background {
            defaults.boolForKey(key)
        } else {
            defaults.removeObjectForKey(key);
            false
        }
    }

    pub(super) fn enter_lightweight() {
        NSUserDefaults::standardUserDefaults().setBool_forKey(true, ns_string!("lightweightMode"));
    }

    fn home() -> anyhow::Result<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .context("resolve macOS home directory")
    }

    fn installed_executable(home: &Path) -> anyhow::Result<Option<PathBuf>> {
        let executable = std::env::current_exe()?.canonicalize()?;
        // A DMG, translocated app or development build must not claim login.
        let installed = executable.ends_with("Contents/MacOS/codex-mixin-desktop")
            && (executable.starts_with("/Applications")
                || executable.starts_with(home.join("Applications")));
        Ok(installed.then_some(executable))
    }

    pub(super) fn handoff_desktop_login() -> anyhow::Result<()> {
        let home = home()?;
        let Some(executable) = installed_executable(&home)? else {
            return Ok(());
        };

        let agents = home.join("Library/LaunchAgents");
        let path = agents.join(format!("{MENU_LABEL}.plist"));
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        // Edit a private sibling and replace the original only after validation.
        let mut staged = tempfile::NamedTempFile::new_in(&agents)?;
        let original = fs::read(&path)?;
        staged.write_all(&original)?;
        let output = Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-", "--"])
            .arg(staged.path())
            .output()?;
        ensure!(
            output.status.success(),
            "read existing login item: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let plist: Value = serde_json::from_slice(&output.stdout)?;
        if plist["Label"].as_str() != Some(MENU_LABEL)
            || plist.get("Program").is_some()
            || plist.get("BundleProgram").is_some()
        {
            return Ok(());
        }
        let Some([program, mode]) = plist["ProgramArguments"].as_array().map(Vec::as_slice) else {
            return Ok(());
        };
        let Some(previous) = program.as_str().map(Path::new) else {
            return Ok(());
        };
        if mode.as_str() != Some("--background")
            || previous == executable
            || !previous.is_absolute()
            || !(previous.ends_with("Contents/MacOS/CodexMixinMenu")
                || previous.ends_with("Contents/MacOS/codex-mixin-desktop"))
        {
            return Ok(());
        }
        let output = Command::new("/usr/bin/plutil")
            .args(["-replace", "ProgramArguments.0", "-string"])
            .arg(&executable)
            .arg("--")
            .arg(staged.path())
            .output()?;
        ensure!(
            output.status.success(),
            "update existing login item: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        // The original label, activation policy and any launchctl disable
        // override carry forward to the next login.
        fs::set_permissions(staged.path(), metadata.permissions())?;
        if fs::read(&path)? == original {
            staged.persist(&path)?;
        }
        Ok(())
    }
}
