//! Operating-system and architecture names used by vendor download channels.

/// Go-style target names (`darwin`/`linux`/`windows`, `amd64`/`arm64`), as
/// used by the DUCX package channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageTarget {
    pub os: &'static str,
    pub arch: &'static str,
}

pub fn package_target() -> anyhow::Result<PackageTarget> {
    package_target_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn package_target_for(os: &str, arch: &str) -> anyhow::Result<PackageTarget> {
    let os = match os {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        other => anyhow::bail!("no vendor packages are published for OS {other}"),
    };
    let arch = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => anyhow::bail!("no vendor packages are published for architecture {other}"),
    };
    Ok(PackageTarget { os, arch })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_rust_targets_to_go_style_names() {
        assert_eq!(
            package_target_for("macos", "aarch64").unwrap(),
            PackageTarget {
                os: "darwin",
                arch: "arm64"
            }
        );
        assert_eq!(
            package_target_for("windows", "x86_64").unwrap(),
            PackageTarget {
                os: "windows",
                arch: "amd64"
            }
        );
        assert!(package_target_for("freebsd", "x86_64").is_err());
    }
}
