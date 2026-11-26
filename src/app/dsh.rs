//! Keep DSH's reporting hooks bridge installed and mounted.
//!
//! DSH 0.2.x does not read `hooks.json` on its own: the official
//! `@deepseek-ai/dsh-hooks-claude-code` bridge plugin has to be installed into
//! the profile and mounted from `cordis.patch.yml`, otherwise every reporting
//! hook stays inert. The profile YAML belongs to `clients::dsh`; this module
//! owns the install/mount order and calls the client adapter.

use std::path::Path;
use std::process::Command;

use crate::clients::dsh;

/// Install the bridge when needed and mount it in every existing profile.
pub fn sync_report_hooks(dsh_home: &Path, hooks_path: &Path) -> anyhow::Result<()> {
    for profile in dsh::report_hook_profiles(dsh_home)? {
        let installed = std::iter::once(dsh::REPORT_HOOKS_PLUGIN)
            .chain(dsh::REPORT_HOOKS_PLUGIN_PEERS)
            .all(|package| dsh::plugin_installed(dsh_home, &profile.directory, package));
        if !installed {
            install_plugin(&profile.name);
        }
        if !dsh::plugin_installed(dsh_home, &profile.directory, dsh::REPORT_HOOKS_PLUGIN) {
            tracing::warn!(
                profile = %profile.name,
                plugin = dsh::REPORT_HOOKS_PLUGIN,
                "DSH reporting hooks stay disabled: bridge plugin is not installed",
            );
            continue;
        }
        dsh::mount_report_hooks(&profile.patch_path, hooks_path)?;
    }
    Ok(())
}

/// Remove the managed row from every profile, keeping the plugin installed.
pub fn remove_report_hooks(dsh_home: &Path) -> anyhow::Result<()> {
    for profile in dsh::report_hook_profiles(dsh_home)? {
        dsh::unmount_report_hooks(&profile.patch_path)?;
    }
    Ok(())
}

fn install_plugin(profile: &str) {
    let Some(cli) = crate::platform::installation::dsh_cli_executable() else {
        return;
    };
    let Some(version) = dsh_version(&cli) else {
        return;
    };
    let mut command = Command::new(&cli);
    command
        .arg("plugin")
        .arg("--profile")
        .arg(profile)
        .arg("add")
        .arg(format!("{}@{version}", dsh::REPORT_HOOKS_PLUGIN));
    for peer in dsh::REPORT_HOOKS_PLUGIN_PEERS {
        command.arg(format!("{peer}@{version}"));
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            tracing::warn!(
                command = %cli.display(),
                profile,
                error = %error,
                "DSH bridge plugin installation could not run",
            );
            return;
        }
    };
    if !output.status.success() {
        tracing::warn!(
            command = %cli.display(),
            profile,
            status = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "DSH bridge plugin installation failed",
        );
    }
}

fn dsh_version(cli: &Path) -> Option<String> {
    let output = Command::new(cli).arg("-V").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!version.is_empty() && version.len() <= 64).then_some(version)
}
