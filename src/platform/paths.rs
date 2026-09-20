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
