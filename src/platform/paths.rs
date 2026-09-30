use std::path::PathBuf;

fn home_variables() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("USERPROFILE", "HOME")
    } else {
        ("HOME", "USERPROFILE")
    }
}

pub fn home_dir() -> PathBuf {
    home_dir_required().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn home_dir_required() -> anyhow::Result<PathBuf> {
    let (primary, secondary) = home_variables();
    std::env::var_os(primary)
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os(secondary).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow::anyhow!("home directory is not set (neither {primary} nor {secondary})")
        })
}

/// Environment variables through which programs find the user's home. Set
/// all of them on a child so tools that read either one agree.
pub(super) const HOME_VARIABLES: [&str; 2] = ["HOME", "USERPROFILE"];

/// Point a child process at an isolated home directory.
pub fn set_home_env(command: &mut std::process::Command, home: &std::path::Path) {
    for variable in HOME_VARIABLES {
        command.env(variable, home);
    }
}

/// Tokio counterpart of [`set_home_env`].
pub fn set_tokio_home_env(command: &mut tokio::process::Command, home: &std::path::Path) {
    for variable in HOME_VARIABLES {
        command.env(variable, home);
    }
}

/// Name of the running operating system for diagnostics and reports.
pub fn os_name() -> &'static str {
    std::env::consts::OS
}
