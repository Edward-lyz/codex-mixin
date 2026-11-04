//! Mount the DSH Claude-hooks bridge plugin.
//!
//! DSH 0.2.x does not read `hooks.json` on its own: the official
//! `@deepseek-ai/dsh-hooks-claude-code` bridge plugin has to be installed into
//! the profile and mounted from `cordis.patch.yml`, otherwise every reporting
//! hook stays inert.
//!
//! The mount is stored as regular YAML rather than a marker-delimited text
//! block because codex-mixin already re-serializes the same patch layer when it
//! syncs the DSH provider, which would strip any comment markers.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use anyhow::Context;
use serde_yaml::{Mapping, Value};

const PLUGIN: &str = "@deepseek-ai/dsh-hooks-claude-code";
/// Peer packages the bridge imports directly. DSH profiles disable pnpm's
/// automatic peer installation, so `dsh plugin add` alone leaves the bridge
/// unable to import and every reporting hook stays inert.
const PLUGIN_PEERS: [&str; 2] = ["@deepseek-ai/dsh-hook-protocol", "@deepseek-ai/dsh-llm"];
const ROW_ID: &str = "codex-mixin-report-hooks";
const PATCH_FILE: &str = "cordis.patch.yml";
const PROFILES_DIRECTORY: &str = "profiles";

/// Install the bridge when needed and mount it in every existing profile.
///
/// Each profile resolves plugins from its own `node_modules`, so the package
/// has to be present in the profile that receives the mount row.
pub(in crate::cli) fn sync(dsh_home: &Path) -> anyhow::Result<()> {
    let hooks_path = dsh_home.join("hooks.json");
    for patch_path in profile_patch_files(dsh_home)? {
        let profile_dir = patch_path.parent();
        let Some(profile) = profile_dir
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        let installed = std::iter::once(PLUGIN)
            .chain(PLUGIN_PEERS)
            .all(|package| package_installed(dsh_home, profile_dir, package));
        if !installed {
            install_plugin(profile)?;
        }
        if !package_installed(dsh_home, profile_dir, PLUGIN) {
            tracing::warn!(
                profile,
                plugin = PLUGIN,
                "DSH reporting hooks stay disabled: bridge plugin is not installed",
            );
            continue;
        }
        mount_patch(&patch_path, &hooks_path)?;
    }
    Ok(())
}

/// Remove the managed row from every profile, keeping the plugin installed.
pub(in crate::cli) fn unmount(dsh_home: &Path) -> anyhow::Result<()> {
    for patch_path in profile_patch_files(dsh_home)? {
        let Some(contents) = read_patch(&patch_path)? else {
            continue;
        };
        let (updated, changed) = without_managed_rows(contents)?;
        if changed {
            write_patch(&patch_path, &updated)?;
        }
    }
    Ok(())
}

fn profile_patch_files(dsh_home: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let profiles = dsh_home.join(PROFILES_DIRECTORY);
    let entries = match std::fs::read_dir(&profiles) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read DSH profiles {}", profiles.display()));
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join(PATCH_FILE);
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn mount_patch(path: &Path, hooks_path: &Path) -> anyhow::Result<()> {
    let contents = read_patch(path)?.unwrap_or_else(|| Value::Sequence(Vec::new()));
    let (mut contents, _) = without_managed_rows(contents)?;
    let entries = contents.as_sequence_mut().with_context(|| {
        format!(
            "DSH profile patch must be a YAML sequence: {}",
            path.display()
        )
    })?;

    let mut config = Mapping::new();
    config.insert(
        Value::String("configPath".to_owned()),
        Value::String(hooks_path.to_string_lossy().into_owned()),
    );
    let mut row = Mapping::new();
    row.insert(
        Value::String("id".to_owned()),
        Value::String(ROW_ID.to_owned()),
    );
    row.insert(
        Value::String("name".to_owned()),
        Value::String(PLUGIN.to_owned()),
    );
    row.insert(Value::String("config".to_owned()), Value::Mapping(config));
    let mut insert = Mapping::new();
    insert.insert(
        Value::String("insert".to_owned()),
        Value::Sequence(vec![Value::Mapping(row)]),
    );
    entries.push(Value::Mapping(insert));
    write_patch(path, &contents)
}

fn without_managed_rows(mut contents: Value) -> anyhow::Result<(Value, bool)> {
    let entries = contents
        .as_sequence_mut()
        .with_context(|| "DSH profile patch must be a YAML sequence to update reporting hooks")?;
    let mut changed = false;
    let mut retained = Vec::with_capacity(entries.len());
    for entry in std::mem::take(entries) {
        match strip_managed_rows(entry) {
            None => changed = true,
            Some(entry) => retained.push(entry),
        }
    }
    *entries = retained;
    Ok((contents, changed))
}

/// Drop `codex-mixin-report-hooks` rows from one patch entry.
///
/// Returns `None` when the entry only carried our insert wrapper, so an
/// unrelated `insert` block owned by the user is preserved.
fn strip_managed_rows(entry: Value) -> Option<Value> {
    let Some(mapping) = entry.as_mapping() else {
        return Some(entry);
    };
    let key = Value::String("insert".to_owned());
    let Some(rows) = mapping.get(&key).and_then(Value::as_sequence) else {
        return Some(entry);
    };
    if !rows.iter().any(is_managed_row) {
        return Some(entry);
    }
    let retained: Vec<Value> = rows
        .iter()
        .filter(|row| !is_managed_row(row))
        .cloned()
        .collect();
    if retained.is_empty() {
        return None;
    }
    let mut mapping = mapping.clone();
    mapping.insert(key, Value::Sequence(retained));
    Some(Value::Mapping(mapping))
}

fn is_managed_row(row: &Value) -> bool {
    row.get("id").and_then(Value::as_str) == Some(ROW_ID)
}

fn read_patch(path: &Path) -> anyhow::Result<Option<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("read DSH patch layer {}", path.display()));
        }
    };
    if text.trim().is_empty() {
        return Ok(Some(Value::Sequence(Vec::new())));
    }
    let contents = serde_yaml::from_str::<Value>(&text)
        .with_context(|| format!("parse DSH patch layer {}", path.display()))?;
    Ok(Some(contents))
}

fn write_patch(path: &Path, contents: &Value) -> anyhow::Result<()> {
    let yaml = serde_yaml::to_string(contents)
        .with_context(|| format!("render DSH patch layer {}", path.display()))?;
    codex_mixin::platform::write_atomic_if_changed(path, yaml.as_bytes())
        .with_context(|| format!("write DSH patch layer {}", path.display()))?;
    codex_mixin::platform::restrict_owner_only_file(path)
        .with_context(|| format!("restrict DSH patch layer {}", path.display()))?;
    Ok(())
}

fn package_installed(dsh_home: &Path, profile_dir: Option<&Path>, package: &str) -> bool {
    let shared = dsh_home
        .join(PROFILES_DIRECTORY)
        .join("node_modules")
        .join(package)
        .join("package.json");
    if shared.is_file() {
        return true;
    }
    profile_dir.is_some_and(|profile| {
        profile
            .join("node_modules")
            .join(package)
            .join("package.json")
            .is_file()
    })
}

fn install_plugin(profile: &str) -> anyhow::Result<()> {
    let Some(cli) = codex_mixin::platform::installation::dsh_cli_executable() else {
        return Ok(());
    };
    let Some(version) = dsh_version(&cli) else {
        return Ok(());
    };
    let mut command = StdCommand::new(&cli);
    command
        .arg("plugin")
        .arg("--profile")
        .arg(profile)
        .arg("add")
        .arg(format!("{PLUGIN}@{version}"));
    for peer in PLUGIN_PEERS {
        command.arg(format!("{peer}@{version}"));
    }
    let output = command
        .output()
        .with_context(|| format!("run {} plugin add", cli.display()))?;
    if !output.status.success() {
        tracing::warn!(
            command = %cli.display(),
            profile,
            status = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "DSH bridge plugin installation failed",
        );
    }
    Ok(())
}

fn dsh_version(cli: &Path) -> Option<String> {
    let output = StdCommand::new(cli).arg("-V").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!version.is_empty() && version.len() <= 64).then_some(version)
}
